use crate::{
    error,
    linux_execve::{exec_linux_file, system_error},
    runner_launch::parse_runner_target_argv,
    runner_protocol::*,
};
use hoh_error::BoxError;
use std::{os::fd::AsRawFd, os::unix::net::UnixStream};

#[cfg(test)]
mod tests;

struct ControlDescriptor {
    _source: rustix::fd::OwnedFd,
    duplicated: bool,
}
impl Drop for ControlDescriptor {
    fn drop(&mut self) {
        if self.duplicated {
            let _ = nix::unistd::close(7);
        }
    }
}
fn control_descriptor(path: &str) -> Result<ControlDescriptor, BoxError> {
    let stream = UnixStream::connect(path)?;
    let source: rustix::fd::OwnedFd = stream.into();
    let duplicated = source.as_raw_fd() != 7;
    if duplicated {
        // The helper is deliberately single-threaded. dup2 atomically replaces an
        // inherited fd 7, exactly like child_process stdio mapping does in Node.
        nix::unistd::dup2(source.as_raw_fd(), 7)?;
    }
    Ok(ControlDescriptor {
        _source: source,
        duplicated,
    })
}
fn exec_target(
    request: &LinuxLaunchRequest,
    argv: &[String],
    control: Option<&ControlDescriptor>,
) -> Result<std::convert::Infallible, BoxError> {
    exec_target_with(request, argv, |file, argv, request| {
        exec_linux_file(
            file,
            argv,
            &request.env,
            &request.cwd,
            control.is_some(),
            request.pipe_fds.as_deref().unwrap_or_default(),
        )
    })
}

fn exec_target_with(
    request: &LinuxLaunchRequest,
    argv: &[String],
    mut execve: impl FnMut(
        &str,
        &[String],
        &LinuxLaunchRequest,
    ) -> Result<std::convert::Infallible, BoxError>,
) -> Result<std::convert::Infallible, BoxError> {
    let file = &argv[0];
    let mut execute = |candidate: &str| {
        let result = execve(candidate, argv, request);
        if result
            .as_ref()
            .err()
            .and_then(|e| e.downcast_ref::<RunnerFailure>())
            .and_then(|e| e.diagnostic().code.as_deref())
            == Some("ENOEXEC")
        {
            let mut shell = vec!["/bin/sh".to_owned(), candidate.to_owned()];
            shell.extend_from_slice(&argv[1..]);
            return execve("/bin/sh", &shell, request);
        }
        result
    };
    if file.contains('/') {
        return execute(file);
    }
    let mut permission = None;
    for directory in request
        .env
        .get("PATH")
        .map_or("/usr/bin:/bin", String::as_str)
        .split(':')
    {
        // Source concatenates here: normalizing link/.. would change symlink traversal.
        let root = if directory.starts_with('/') {
            directory.to_owned()
        } else {
            format!(
                "{}{}{directory}",
                request.cwd,
                if request.cwd.ends_with('/') { "" } else { "/" }
            )
        };
        let candidate = format!("{root}{}{file}", if root.ends_with('/') { "" } else { "/" });
        let failure = match execute(&candidate) {
            Ok(never) => match never {},
            Err(error) => error,
        };
        match failure
            .downcast_ref::<RunnerFailure>()
            .and_then(|e| e.diagnostic().code.as_deref())
        {
            Some("EACCES") => {
                if permission.is_none() {
                    permission = Some(failure);
                }
            }
            Some("ENOENT" | "ENOTDIR") => {}
            _ => return Err(failure),
        }
    }
    Err(permission.unwrap_or_else(|| {
        deserialize_runner_error(SerializedRunnerError {
            name: "Error".into(),
            message: format!("spawn {file} ENOENT"),
            code: Some("ENOENT".into()),
            syscall: Some(format!("spawn {file}")),
            path: Some(file.clone()),
        })
    }))
}
pub fn run_spawn_runner(locator: &str, args: &[String]) -> Result<i32, BoxError> {
    let files = linux_launch_files_from_locator(locator)?;
    let _exec_acknowledgement = acknowledge_exec(&files)?;
    let result = run_spawn_runner_inner(&files, args);
    if let Err(error) = &result {
        write_linux_startup_error(
            &files,
            &LinuxStartupError::Error {
                error: serialize_runner_error(error.as_ref()),
            },
        )?;
    }
    result
}
fn run_spawn_runner_inner(files: &LinuxLaunchFiles, args: &[String]) -> Result<i32, BoxError> {
    let argv = parse_runner_target_argv(args)?;
    let mut request = match consume_linux_launch_request(&files.request_path) {
        Ok(request) => request,
        Err(error) => {
            write_linux_startup_error(
                files,
                &LinuxStartupError::Error {
                    error: serialize_runner_error(error.as_ref()),
                },
            )?;
            return Ok(127);
        }
    };
    let result = (|| {
        // The helper alone changes process-global cwd; no Tokio threads exist here.
        std::env::set_current_dir(&request.cwd)
            .map_err(|e| system_error(e, "chdir", Some(&request.cwd)))?;
        request.cwd = std::env::current_dir()
            .map_err(|e| system_error(e, "uv_cwd", None))?
            .to_string_lossy()
            .into_owned();
        let control = if request.control.is_some() {
            Some(control_descriptor(
                request.control_path.as_deref().ok_or_else(|| {
                    error("subprocess runner received an invalid Linux launch request")
                })?,
            )?)
        } else {
            None
        };
        exec_target(&request, &argv, control.as_ref())
    })();
    match result {
        Ok(never) => match never {},
        Err(error) => {
            let mut serialized = serialize_runner_error(error.as_ref());
            if let Some(code) = serialized.code.clone() {
                serialized.message = format!("spawn {} {code}", argv[0]);
                serialized.syscall = Some(format!("spawn {}", argv[0]));
                serialized.path = Some(argv[0].clone());
            }
            write_linux_startup_error(files, &LinuxStartupError::Error { error: serialized })?;
            Ok(127)
        }
    }
}
pub fn run_selected_subprocess_runner(selection: &str, args: &[String]) -> i32 {
    use crate::runner_launch::{BOOTSTRAP_SESSION_ENV, DETACHED_EXEC_MODE};
    if std::env::var_os(BOOTSTRAP_SESSION_ENV).as_deref() != Some(std::ffi::OsStr::new("inherit")) {
        let _ = rustix::process::setsid();
    }
    let result = if args.first().is_some_and(|mode| mode == DETACHED_EXEC_MODE) {
        run_detached_exec(selection, &args[1..])
    } else {
        run_spawn_runner(selection, args)
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            if let Ok(files) = linux_launch_files_from_locator(selection) {
                let _ = write_linux_startup_error(
                    &files,
                    &LinuxStartupError::Error {
                        error: serialize_runner_error(error.as_ref()),
                    },
                );
            }
            127
        }
    }
}

fn run_detached_exec(locator: &str, args: &[String]) -> Result<i32, BoxError> {
    use std::os::unix::process::CommandExt;
    let files = linux_launch_files_from_locator(locator)?;
    let _acknowledgement = acknowledge_detached_exec(&files)?;
    let result = (|| {
        let argv = parse_runner_target_argv(args)?;
        let error = std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .env(crate::runner_launch::BOOTSTRAP_SESSION_ENV, "inherit")
            .exec();
        Err(deserialize_runner_error(SerializedRunnerError {
            name: "Error".into(),
            message: hoh_error::node_spawn_error_message(&error, &argv[0]),
            code: hoh_error::node_error_code(&error).map(Into::into),
            syscall: Some(format!("spawn {}", argv[0])),
            path: Some(argv[0].clone()),
        }))
    })();
    if let Err(error) = &result {
        write_linux_startup_error(
            &files,
            &LinuxStartupError::Error {
                error: serialize_runner_error(error.as_ref()),
            },
        )?;
    }
    result
}
