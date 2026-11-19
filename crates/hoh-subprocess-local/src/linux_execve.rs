//! Safe exec and descriptor preservation in the one-shot, single-threaded helper.
use crate::runner_protocol::SerializedRunnerError;
use hoh_error::BoxError;
use indexmap::IndexMap;
use rustix::{fd::BorrowedFd, fs::OFlags, io::FdFlags};
use std::os::unix::process::CommandExt;

#[cfg(test)]
mod tests;

trait DescriptorFlags {
    fn getfd(&self, fd: u64) -> Result<FdFlags, BoxError>;
    fn setfd(&self, fd: u64, flags: FdFlags) -> Result<(), BoxError>;
    fn getfl(&self, fd: u64) -> Result<OFlags, BoxError>;
    fn setfl(&self, fd: u64, flags: OFlags) -> Result<(), BoxError>;
}
struct NativeFlags(bool);
impl NativeFlags {
    fn fd(&self, fd: u64) -> Result<BorrowedFd<'_>, BoxError> {
        match fd {
            0 => Ok(rustix::stdio::stdin()),
            1 => Ok(rustix::stdio::stdout()),
            2 => Ok(rustix::stdio::stderr()),
            7 if !self.0 => Err(crate::error("control descriptor is absent")),
            _ => Err(crate::error("invalid child-owned pipe descriptor")),
        }
    }
    fn fcntl_error(error: nix::errno::Errno) -> BoxError {
        system_error(
            std::io::Error::from_raw_os_error(error as i32),
            "fcntl",
            None,
        )
    }
}
impl DescriptorFlags for NativeFlags {
    fn getfd(&self, fd: u64) -> Result<FdFlags, BoxError> {
        if fd == 7 && self.0 {
            return nix::fcntl::fcntl(7, nix::fcntl::FcntlArg::F_GETFD)
                .map(|flags| FdFlags::from_bits_retain(flags as u32))
                .map_err(Self::fcntl_error);
        }
        rustix::io::fcntl_getfd(self.fd(fd)?).map_err(|e| system_error(e.into(), "fcntl", None))
    }
    fn setfd(&self, fd: u64, flags: FdFlags) -> Result<(), BoxError> {
        if fd == 7 && self.0 {
            return nix::fcntl::fcntl(
                7,
                nix::fcntl::FcntlArg::F_SETFD(nix::fcntl::FdFlag::from_bits_retain(
                    flags.bits() as i32
                )),
            )
            .map(|_| ())
            .map_err(Self::fcntl_error);
        }
        rustix::io::fcntl_setfd(self.fd(fd)?, flags)
            .map_err(|e| system_error(e.into(), "fcntl", None))
    }
    fn getfl(&self, fd: u64) -> Result<OFlags, BoxError> {
        rustix::fs::fcntl_getfl(self.fd(fd)?).map_err(|e| system_error(e.into(), "fcntl", None))
    }
    fn setfl(&self, fd: u64, flags: OFlags) -> Result<(), BoxError> {
        rustix::fs::fcntl_setfl(self.fd(fd)?, flags)
            .map_err(|e| system_error(e.into(), "fcntl", None))
    }
}
fn prepare_descriptors(
    flags: &impl DescriptorFlags,
    control: bool,
    pipe_fds: &[u64],
) -> Result<(), BoxError> {
    for fd in [0, 1, 2].into_iter().chain(control.then_some(7)) {
        let previous = flags.getfd(fd)?;
        if previous.contains(FdFlags::CLOEXEC) {
            flags.setfd(fd, previous - FdFlags::CLOEXEC)?;
        }
    }
    for &fd in pipe_fds {
        let previous = flags.getfl(fd)?;
        if previous.contains(OFlags::NONBLOCK) {
            flags.setfl(fd, previous - OFlags::NONBLOCK)?;
        }
    }
    Ok(())
}
pub fn system_error(error: std::io::Error, syscall: &str, path: Option<&str>) -> BoxError {
    let code = hoh_error::node_error_code(&error).map(str::to_owned);
    crate::runner_protocol::deserialize_runner_error(SerializedRunnerError {
        name: "Error".into(),
        message: hoh_error::node_fs_error_message(&error, syscall, path, None),
        code,
        syscall: Some(syscall.into()),
        path: path.map(Into::into),
    })
}
pub fn exec_linux_file(
    file: &str,
    argv: &[String],
    env: &IndexMap<String, String>,
    _cwd: &str,
    control: bool,
    pipe_fds: &[u64],
) -> Result<std::convert::Infallible, BoxError> {
    prepare_descriptors(&NativeFlags(control), control, pipe_fds)?;
    let mut command = std::process::Command::new(file);
    command.arg0(argv.first().map_or(file, String::as_str));
    command.args(argv.iter().skip(1));
    command.env_clear().envs(env);
    Err(system_error(command.exec(), "execve", Some(file)))
}
