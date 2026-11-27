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
#[cfg(target_os = "linux")]
use std::{
    ffi::{CStr, CString},
    os::{
        fd::{FromRawFd, OwnedFd},
        unix::fs::MetadataExt,
    },
};
use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, Read, Write},
    mem::size_of,
    net::Shutdown,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{FileExt, PermissionsExt},
            net::UnixStream,
            process::CommandExt,
        },
    },
    path::PathBuf,
    process::{Child, Command},
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
#[cfg(target_os = "linux")]
const START_TIME_OFFSET: u64 = (STATUS_PID_SIZE + BOOT_ID_SIZE) as u64;
#[cfg(target_os = "linux")]
const NAMESPACE_OFFSET: u64 = START_TIME_OFFSET + size_of::<u64>() as u64;
const GUARD_CHECK: u8 = 1;
const GUARD_CHECK_TIMEOUT: Duration = Duration::from_millis(250);
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
        let (control, mut owner) = UnixStream::pair().map_err(guard_error)?;
        guard_timeouts(&owner).map_err(guard_error)?;
        let executable = std::env::current_exe().map_err(guard_error)?;
        let mut command = Command::new(executable);
        command.arg(ENGINE_GUARD).process_group(0);
        pass_fd(&mut command, &control).map_err(guard_error)?;
        pass_fd(&mut command, &*self.lifecycle).map_err(guard_error)?;
        pass_fd(&mut command, &self.lease).map_err(guard_error)?;
        pass_fd(&mut command, &self.status).map_err(guard_error)?;
        command.arg(&self.config.path).args(&self.config.args);
        let confirm = Box::new(move || {
            let mut reply = [0];
            if owner.write_all(&[GUARD_CHECK]).is_err() || owner.read_exact(&mut reply).is_err() {
                let _ = owner.shutdown(Shutdown::Both);
                return false;
            }
            reply[0] == GUARD_CHECK
        });
        self.config
            .connect_process(command, Some(confirm), generation, inbox)
    }
}

fn guard_error(error: io::Error) -> String {
    format!("Could not prepare the HoH update engine ({error}).")
}

fn guard_timeouts(socket: &UnixStream) -> io::Result<()> {
    socket.set_read_timeout(Some(GUARD_CHECK_TIMEOUT))?;
    socket.set_write_timeout(Some(GUARD_CHECK_TIMEOUT))
}

pub(super) fn run_guard(mut args: impl Iterator<Item = OsString>) -> io::Result<()> {
    let mut control = UnixStream::from(take_fd(args.next().as_deref())?);
    guard_timeouts(&control)?;
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
            let started = read_process_start_time(c"/proc/self/stat")?;
            status.write_all_at(&started.to_ne_bytes(), START_TIME_OFFSET)?;
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
        if result > 0 {
            if !reply_to_guard_check(&mut control, &mut engine).unwrap_or(false) {
                break;
            }
        } else if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            break;
        }
    }
    kill_and_wait(&mut engine)?;
    record_engine_status(&status, 0)
}

fn reply_to_guard_check(control: &mut UnixStream, engine: &mut Child) -> io::Result<bool> {
    let mut request = [0];
    control.read_exact(&mut request)?;
    if request[0] != GUARD_CHECK {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let running = !process_exited(engine);
    control.write_all(&[u8::from(running)])?;
    Ok(running)
}

pub(super) fn initialize_engine_status(status: &File) -> io::Result<()> {
    if let Some(boot) = boot_id().filter(valid_boot_id) {
        status.write_all_at(&boot, STATUS_PID_SIZE as u64)?;
    } else if cfg!(target_os = "linux") {
        return Err(unconfirmed_engine_exit());
    }
    #[cfg(target_os = "linux")]
    status.write_all_at(&process_namespaces()?, NAMESPACE_OFFSET)?;
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
    status.read_exact_at(&mut recorded, STATUS_PID_SIZE as u64)?;
    let current = boot_id()
        .filter(valid_boot_id)
        .ok_or_else(unconfirmed_engine_exit)?;
    if !valid_boot_id(&recorded) {
        return Err(unconfirmed_engine_exit());
    }
    if !recorded.eq_ignore_ascii_case(&current) {
        return Ok(());
    }
    await_engine_stop(status, Instant::now())
}

fn valid_boot_id(id: &[u8; BOOT_ID_SIZE]) -> bool {
    id.iter().enumerate().all(|(index, byte)| match index {
        8 | 13 | 18 | 23 => *byte == b'-',
        _ => byte.is_ascii_hexdigit(),
    })
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

#[cfg(target_os = "macos")]
pub(super) fn await_engine_stop(status: &File, _deadline: Instant) -> io::Result<()> {
    require_engine_stopped(status)
}

#[cfg(target_os = "linux")]
pub(super) fn await_engine_stop(status: &File, deadline: Instant) -> io::Result<()> {
    let pid = engine_status(status)?;
    if pid == 0 {
        return Ok(());
    }
    let mut started = [0; size_of::<u64>()];
    status.read_exact_at(&mut started, START_TIME_OFFSET)?;
    let started = u64::from_ne_bytes(started);
    let mut namespaces = [0; 16];
    status.read_exact_at(&mut namespaces, NAMESPACE_OFFSET)?;
    if pid > 0
        && started > 0
        && namespaces == process_namespaces()?
        && process_has_exited(pid, started, deadline)?
    {
        return Ok(());
    }
    Err(unconfirmed_engine_exit())
}

#[cfg(target_os = "linux")]
fn process_namespaces() -> io::Result<[u8; 16]> {
    let pid = fs::metadata("/proc/self/ns/pid")?.ino();
    let time = match fs::metadata("/proc/self/ns/time") {
        Ok(namespace) => namespace.ino(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error),
    };
    let mut namespaces = [0; 16];
    namespaces[..8].copy_from_slice(&pid.to_ne_bytes());
    namespaces[8..].copy_from_slice(&time.to_ne_bytes());
    Ok(namespaces)
}

#[cfg(target_os = "linux")]
fn read_process_start_time(path: &CStr) -> io::Result<u64> {
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    let mut buffer = [0; 1024];
    let size = loop {
        match file.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => break result?,
        }
    };
    process_start_time(&buffer[..size]).ok_or_else(|| io::ErrorKind::InvalidData.into())
}

#[cfg(target_os = "linux")]
fn process_start_time(stat: &[u8]) -> Option<u64> {
    let end_of_name = stat.iter().rposition(|&byte| byte == b')')?;
    let mut fields = stat[end_of_name + 1..]
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|field| !field.is_empty());
    let started = fields.nth(19)?;
    fields.next()?;
    std::str::from_utf8(started).ok()?.parse().ok()
}

#[cfg(target_os = "linux")]
fn process_has_exited(pid: libc::pid_t, started: u64, deadline: Instant) -> io::Result<bool> {
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
    let stat = CString::new(format!("/proc/{pid}/stat"))?;
    match read_process_start_time(&stat) {
        Ok(current) if current > started => Ok(true),
        Ok(current) if current == started => poll_process_exit(&fd, deadline),
        Ok(_) => Err(unconfirmed_engine_exit()),
        Err(_) if poll_process_exit(&fd, Instant::now())? => Ok(true),
        Err(error) => Err(error),
    }
}

#[cfg(target_os = "linux")]
fn poll_process_exit(fd: &OwnedFd, deadline: Instant) -> io::Result<bool> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    fn engine_waiting_on_stdin() -> Child {
        Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap()
    }

    #[test]
    fn guard_checks_the_engine_even_when_the_control_socket_is_open() {
        let mut engine = engine_waiting_on_stdin();
        let (mut owner, mut control) = UnixStream::pair().unwrap();
        guard_timeouts(&owner).unwrap();
        guard_timeouts(&control).unwrap();
        let mut reply = [0];

        owner.write_all(&[GUARD_CHECK]).unwrap();
        assert!(reply_to_guard_check(&mut control, &mut engine).unwrap());
        owner.read_exact(&mut reply).unwrap();
        assert_eq!(reply, [GUARD_CHECK]);

        drop(engine.stdin.take());
        engine.wait().unwrap();
        owner.write_all(&[GUARD_CHECK]).unwrap();
        assert!(!reply_to_guard_check(&mut control, &mut engine).unwrap());
        owner.read_exact(&mut reply).unwrap();
        assert_eq!(reply, [0]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn same_boot_recovery_requires_the_recorded_engine_to_have_exited() {
        let status = tempfile::tempfile().unwrap();
        initialize_engine_status(&status).unwrap();
        let mut engine = engine_waiting_on_stdin();
        let pid = engine.id() as libc::pid_t;
        let stat = CString::new(format!("/proc/{pid}/stat")).unwrap();
        let started = read_process_start_time(&stat).unwrap();

        status
            .write_all_at(&started.to_ne_bytes(), START_TIME_OFFSET)
            .unwrap();
        record_engine_status(&status, pid).unwrap();
        assert!(confirm_interrupted_engine_stopped(&status).is_err());

        status
            .write_all_at(&(started - 1).to_ne_bytes(), START_TIME_OFFSET)
            .unwrap();
        confirm_interrupted_engine_stopped(&status).unwrap();

        status
            .write_all_at(&started.to_ne_bytes(), START_TIME_OFFSET)
            .unwrap();
        drop(engine.stdin.take());
        let stopped = await_engine_stop(&status, Instant::now() + Duration::from_secs(2));
        engine.wait().unwrap();
        stopped.unwrap();
        confirm_interrupted_engine_stopped(&status).unwrap();
    }
}
