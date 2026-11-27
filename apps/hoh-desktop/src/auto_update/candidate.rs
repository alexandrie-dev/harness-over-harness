use super::instance::{pass_fd, take_fd};
use crate::{
    engine::{
        Connector, Message, ProcessConfig, Transport, kill_and_wait, process_exited,
        request_shutdown, valid_initialize_result,
    },
    native::{engine_approval_key, engine_initialize, staged_engine_config},
};
use hoh_protocol::{framing, transport::Incoming};
use serde_json::json;
use std::{
    ffi::OsString,
    fs::{self, File},
    io,
    mem::size_of,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{FileExt, PermissionsExt},
            net::UnixStream,
            process::CommandExt,
        },
    },
    path::PathBuf,
    process::Command,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

pub(super) const ENGINE_GUARD: &str = "--hoh-update-engine-guard";
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_STABILITY: Duration = Duration::from_secs(1);
const PROBE_POLL: Duration = Duration::from_millis(50);
const PROBE_INITIALIZE: &str = "update-probe-initialize";
const STATUS_PID_SIZE: usize = size_of::<libc::pid_t>();
const BOOT_ID_SIZE: usize = 36;
#[cfg(target_os = "macos")]
const ENGINE_UNCONFIRMED: libc::pid_t = -1;

pub(super) struct CandidateEngine {
    pub config: ProcessConfig,
    pub lifecycle: Arc<UnixStream>,
    pub lease: File,
    pub status: File,
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
        pass_fd(&mut command, &self.status).map_err(guard_error)?;
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
    let status = File::from(take_fd(args.next().as_deref())?);
    require_engine_stopped(&status)?;
    let executable = args.next().ok_or(io::ErrorKind::InvalidInput)?;
    let mut command = Command::new(executable);
    command.args(args).process_group(0);
    #[cfg(target_os = "linux")]
    unsafe {
        let guard_pid = libc::getpid();
        let status = status.try_clone()?;
        command.pre_exec(move || {
            if libc::prctl(
                libc::PR_SET_PDEATHSIG,
                libc::SIGKILL as libc::c_ulong,
                0 as libc::c_ulong,
                0 as libc::c_ulong,
                0 as libc::c_ulong,
            ) < 0
            {
                return Err(io::Error::last_os_error());
            }
            if libc::getppid() != guard_pid {
                return Err(io::ErrorKind::Interrupted.into());
            }
            record_engine_status(&status, libc::getpid())
        });
    }
    #[cfg(target_os = "macos")]
    record_engine_status(&status, ENGINE_UNCONFIRMED)?;
    let mut engine = match command.spawn() {
        Ok(engine) => engine,
        Err(error) => {
            eprintln!("Could not start the HoH update engine: {error}");
            return record_engine_status(&status, 0);
        }
    };
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
    kill_and_wait(&mut engine)?;
    record_engine_status(&status, 0)
}

pub(super) fn initialize_engine_status(status: &File) -> io::Result<()> {
    if let Some(boot) = boot_id() {
        status.write_all_at(&boot, STATUS_PID_SIZE as u64)?;
    }
    record_engine_status(status, 0)
}

fn record_engine_status(status: &File, pid: libc::pid_t) -> io::Result<()> {
    status.write_all_at(&pid.to_ne_bytes(), 0)?;
    status.sync_all()
}

fn engine_status(status: &File) -> io::Result<libc::pid_t> {
    let mut bytes = [0; STATUS_PID_SIZE];
    status.read_exact_at(&mut bytes, 0)?;
    Ok(libc::pid_t::from_ne_bytes(bytes))
}

fn require_engine_stopped(status: &File) -> io::Result<()> {
    if engine_status(status)? == 0 {
        return Ok(());
    }
    Err(unconfirmed_engine_exit())
}

pub(super) fn confirm_interrupted_engine_stopped(status: &File) -> io::Result<()> {
    if engine_status(status)? == 0 {
        return Ok(());
    }
    let mut recorded = [0; BOOT_ID_SIZE];
    if let Some(current) = boot_id()
        && status.read_exact_at(&mut recorded, STATUS_PID_SIZE as u64).is_ok()
        && recorded != current
    {
        return Ok(());
    }
    Err(unconfirmed_engine_exit())
}

#[cfg(target_os = "linux")]
fn boot_id() -> Option<[u8; BOOT_ID_SIZE]> {
    let id = fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    id.trim().as_bytes().try_into().ok()
}

#[cfg(target_os = "macos")]
fn boot_id() -> Option<[u8; BOOT_ID_SIZE]> {
    let mut id = [0; BOOT_ID_SIZE + 1];
    let mut length = id.len();
    let result = unsafe {
        libc::sysctlbyname(
            c"kern.bootsessionuuid".as_ptr(),
            id.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 || length != id.len() || id[BOOT_ID_SIZE] != 0 {
        return None;
    }
    id[..BOOT_ID_SIZE].try_into().ok()
}

fn unconfirmed_engine_exit() -> io::Error {
    io::Error::other("could not confirm that the updated HoH engine stopped")
}

pub(super) fn engine_is_running(status: &File) -> bool {
    let Ok(pid) = engine_status(status) else {
        return false;
    };
    #[cfg(target_os = "macos")]
    {
        pid == ENGINE_UNCONFIRMED
    }
    #[cfg(target_os = "linux")]
    {
        if pid <= 0 {
            return false;
        }
        match process_has_exited(pid, Instant::now()) {
            Ok(exited) => !exited,
            Err(error) => matches!(error.raw_os_error(), Some(libc::ENOSYS | libc::EPERM)),
        }
    }
}

#[cfg(target_os = "macos")]
pub(super) fn await_engine_stop(status: &File, _deadline: Instant) -> io::Result<()> {
    require_engine_stopped(status)
}

#[cfg(target_os = "linux")]
pub(super) fn await_engine_stop(status: &File, deadline: Instant) -> io::Result<()> {
    let pid = engine_status(status)?;
    if pid == 0 || pid > 0 && process_has_exited(pid, deadline)? {
        return Ok(());
    }
    Err(unconfirmed_engine_exit())
}

#[cfg(target_os = "linux")]
fn process_has_exited(pid: libc::pid_t, deadline: Instant) -> io::Result<bool> {
    use std::os::fd::{FromRawFd, OwnedFd};

    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(true)
        } else {
            Err(error)
        };
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd as libc::c_int) };
    let mut descriptor = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let result = unsafe { libc::poll(&mut descriptor, 1, remaining.as_millis() as libc::c_int) };
        if result >= 0 {
            return Ok(descriptor.revents & (libc::POLLIN | libc::POLLHUP) != 0);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted || Instant::now() >= deadline {
            return Err(error);
        }
    }
}

pub(super) fn qualify(executable: PathBuf, cancelled: impl Fn() -> bool) -> Result<(), String> {
    let cancelled_error = "The update was cancelled.";
    if cancelled() {
        return Err(cancelled_error.into());
    }
    let prepare_error = |error| format!("Could not prepare the staged HoH engine probe ({error}).");
    let root = tempfile::Builder::new()
        .prefix("hoh-update-probe-")
        .tempdir()
        .map_err(prepare_error)?;
    let path = root.path().canonicalize().map_err(prepare_error)?;
    let workspace = path.join("workspace");
    let home = path.join("engine");
    let scratch = path.join("tmp");
    for directory in [&workspace, &home, &scratch] {
        fs::create_dir(directory).map_err(prepare_error)?;
    }
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).map_err(prepare_error)?;
    let config = staged_engine_config(executable, &home)?;
    let key = engine_approval_key().map_err(prepare_error)?;
    let mut command = Command::new(&config.path);
    command
        .args(&config.args)
        .env_clear()
        .env("HOME", &path)
        .env("TMPDIR", &scratch)
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .process_group(0);
    if cancelled() {
        return Err(cancelled_error.into());
    }
    let (inbox, incoming) = mpsc::channel();
    let mut transport = config.connect_process(command, None, 1, inbox)?;
    let result = transport
        .send(framing::request(
            json!(PROBE_INITIALIZE),
            "initialize",
            engine_initialize(&workspace, &config, &key),
        ))
        .and_then(|()| await_probe(&mut *transport, incoming, &cancelled));
    if result.is_ok() {
        request_shutdown(&mut *transport, "update-probe-shutdown", &cancelled);
    }
    if !transport.close_confirmed() {
        let _ = root.keep();
        return Err("Could not confirm that the staged HoH engine stopped.".into());
    }
    if cancelled() {
        return Err(cancelled_error.into());
    }
    result
}

fn await_probe(
    transport: &mut dyn Transport,
    incoming: mpsc::Receiver<Message>,
    cancelled: impl Fn() -> bool,
) -> Result<(), String> {
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let initialize_key = format!("s:{PROBE_INITIALIZE}");
    let mut ready_at: Option<Instant> = None;
    while !cancelled() && Instant::now() < deadline {
        if transport.exited() || transport.disconnected() {
            break;
        }
        if ready_at.is_some_and(|at| at.elapsed() >= PROBE_STABILITY) {
            return Ok(());
        }
        match incoming.recv_timeout(PROBE_POLL) {
            Ok(Message::Incoming(1, Incoming::Response { id, result, error }))
                if ready_at.is_none() && id.key() == initialize_key =>
            {
                if error.is_some() || !valid_initialize_result(&result) {
                    break;
                }
                ready_at = Some(Instant::now());
            }
            Ok(Message::Disconnected(1)) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            _ => {}
        }
    }
    Err("The staged HoH engine did not initialize successfully.".into())
}
