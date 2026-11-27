use super::instance::{pass_fd, take_fd};
use crate::engine::{Connector, Message, ProcessConfig, Transport, kill_and_wait, process_exited};
use std::{
    ffi::OsString,
    fs::File,
    io,
    os::{
        fd::AsRawFd,
        unix::{net::UnixStream, process::CommandExt},
    },
    process::Command,
    sync::{Arc, mpsc},
};

pub(super) const ENGINE_GUARD: &str = "--hoh-update-engine-guard";

pub(super) struct CandidateEngine {
    pub config: ProcessConfig,
    pub lifecycle: Arc<UnixStream>,
    pub lease: File,
}

impl Connector for CandidateEngine {
    fn connect(
        &self,
        generation: u64,
        inbox: mpsc::Sender<Message>,
    ) -> Result<Box<dyn Transport>, String> {
        let (control, owner) = io::pipe().map_err(guard_error)?;
        let executable = std::env::current_exe().map_err(guard_error)?;
        let mut command = Command::new(executable);
        command.arg(ENGINE_GUARD).process_group(0);
        pass_fd(&mut command, &control).map_err(guard_error)?;
        pass_fd(&mut command, &*self.lifecycle).map_err(guard_error)?;
        pass_fd(&mut command, &self.lease).map_err(guard_error)?;
        command.arg(&self.config.path).args(&self.config.args);
        self.config
            .connect_process(command, Some(owner), generation, inbox)
    }
}

fn guard_error(error: io::Error) -> String {
    format!("Could not prepare the HoH update engine ({error}).")
}

pub(super) fn run_guard(mut args: impl Iterator<Item = OsString>) -> io::Result<()> {
    let control = take_fd(args.next().as_deref())?;
    let _lifecycle = take_fd(args.next().as_deref())?;
    let _lease = take_fd(args.next().as_deref())?;
    let executable = args.next().ok_or(io::ErrorKind::InvalidInput)?;
    let mut engine = Command::new(executable)
        .args(args)
        .process_group(0)
        .spawn()?;
    unsafe {
        libc::close(libc::STDIN_FILENO);
        libc::close(libc::STDOUT_FILENO);
    }
    let mut descriptor = libc::pollfd {
        fd: control.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    while !process_exited(&mut engine) {
        let result = unsafe { libc::poll(&mut descriptor, 1, 50) };
        if result > 0 || result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted
        {
            break;
        }
    }
    kill_and_wait(&mut engine);
    Ok(())
}
