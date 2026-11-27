use super::instance::{inherit_fd, pass_fd, take_fd};
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
    os::{
        fd::AsRawFd,
        unix::{fs::PermissionsExt, net::UnixStream, process::CommandExt},
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
        let (lifetime, engine_end) = io::pipe().map_err(guard_error)?;
        let executable = std::env::current_exe().map_err(guard_error)?;
        let mut command = Command::new(executable);
        command.arg(ENGINE_GUARD).process_group(0);
        pass_fd(&mut command, &control).map_err(guard_error)?;
        pass_fd(&mut command, &*self.lifecycle).map_err(guard_error)?;
        pass_fd(&mut command, &self.lease).map_err(guard_error)?;
        pass_fd(&mut command, &engine_end).map_err(guard_error)?;
        drop(engine_end);
        command.arg(&self.config.path).args(&self.config.args);
        self.config
            .connect_process(command, Some((owner, lifetime)), generation, inbox)
    }
}

fn guard_error(error: io::Error) -> String {
    format!("Could not prepare the HoH update engine ({error}).")
}

pub(super) fn run_guard(mut args: impl Iterator<Item = OsString>) -> io::Result<()> {
    let control = take_fd(args.next().as_deref())?;
    let lifecycle = take_fd(args.next().as_deref())?;
    let lease = take_fd(args.next().as_deref())?;
    let lifetime = take_fd(args.next().as_deref())?;
    let executable = args.next().ok_or(io::ErrorKind::InvalidInput)?;
    let mut command = Command::new(executable);
    command.args(args).process_group(0);
    inherit_fd(&mut command, &lifecycle)?;
    inherit_fd(&mut command, &lease)?;
    inherit_fd(&mut command, &lifetime)?;
    #[cfg(target_os = "linux")]
    unsafe {
        let guard_pid = libc::getpid();
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
            Ok(())
        });
    }
    let mut engine = command.spawn()?;
    drop(command);
    drop(lifetime);
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
    kill_and_wait(&mut engine)
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
