use super::{
    Package,
    candidate::{
        CandidateEngine, ENGINE_GUARD, await_engine_stop, confirm_interrupted_engine_stopped,
        run_guard,
    },
    instance::{InstanceLock, pass_fd, take_fd},
    open_regular_file, package_for, read_install_metadata,
    recovery::{Decision, Recovery, is_backup, sibling_path},
};
use crate::engine::{Connector, ProcessConfig};
use semver::Version;
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, TryLockError},
    io::{self, Read, Write},
    net::Shutdown,
    os::{
        fd::{AsFd, AsRawFd, OwnedFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, PermissionsExt},
            net::UnixStream,
        },
    },
    path::Path,
    process::{Child, ExitStatus, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use tauri::Manager;

const WORKER: &str = "--hoh-update-worker";
const CANDIDATE: &str = "--hoh-update-candidate-fd";
const RECOVERED: &str = "--hoh-update-recovered";
const START_TIMEOUT: Duration = Duration::from_secs(45);
const START_STABILITY: Duration = Duration::from_secs(1);
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
const HELPER_STOP_TIMEOUT: Duration = Duration::from_secs(5);
const WAIT_STEP: Duration = Duration::from_millis(75);
const CHECK_READY: u8 = 1;
const COMMIT_COMPLETE: u8 = 2;
const USER_QUIT: u64 = u64::MAX;
const SHUTDOWN_COMPLETE: u8 = 1;

#[derive(Default)]
enum Job {
    #[default]
    Idle,
    Installing,
    Cancelling,
    Restarting {
        until_exit: UnixStream,
    },
}

pub struct Installation {
    instance: InstanceLock,
    lease: Option<File>,
    job: Mutex<Job>,
    readiness: Option<Arc<CandidateReadiness>>,
    recovered: Mutex<bool>,
}

struct CandidateReadiness {
    lifecycle: Arc<UnixStream>,
    recovery: Recovery,
    engine_status: File,
    phase: Mutex<CandidatePhase>,
    ui: Mutex<Option<(u32, Instant)>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CandidatePhase {
    Waiting,
    Checking,
    Ready,
    Committed,
    Quitting,
    Aborted,
}

fn monitor_worker(mut lifecycle: &UnixStream, phase: &Mutex<CandidatePhase>) -> bool {
    let mut request = [0];
    loop {
        let received = lifecycle.read_exact(&mut request).is_ok();
        let mut phase = phase.lock().unwrap_or_else(|error| error.into_inner());
        if *phase == CandidatePhase::Quitting {
            return true;
        }
        match (received, request[0], *phase) {
            (true, CHECK_READY, CandidatePhase::Waiting) => *phase = CandidatePhase::Checking,
            (true, COMMIT_COMPLETE, CandidatePhase::Ready) => {
                *phase = CandidatePhase::Committed;
                return true;
            }
            _ => {
                *phase = CandidatePhase::Aborted;
                return false;
            }
        }
    }
}

fn update_gate(installed: &Path) -> io::Result<File> {
    let path = sibling_path(installed, "-update.lock")?;
    let gate = match open_regular_file(&path) {
        Ok(gate) => gate,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
            let new_gate = tempfile::NamedTempFile::new_in(parent)?;
            new_gate
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o644))?;
            match new_gate.persist_noclobber(&path) {
                Ok(_) => {}
                Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.error),
            }
            open_regular_file(&path)?
        }
        Err(error) => return Err(error),
    };
    if gate.metadata()?.len() != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the HoH update gate must be empty",
        ));
    }
    gate.try_lock()?;
    Ok(gate)
}

fn check_lease(package: Package, installed: &Path, lease: &File) -> io::Result<()> {
    let active = package.lease_path(installed).symlink_metadata()?;
    let held = lease.metadata()?;
    if (active.dev(), active.ino()) == (held.dev(), held.ino()) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the installation changed while acquiring its lease",
        ))
    }
}

impl Installation {
    pub fn startup(version: &Version) -> Result<Option<Self>, String> {
        let mut args = std::env::args_os().skip(1);
        let mode = args.next();
        let mode = mode.as_deref().and_then(OsStr::to_str);
        if mode == Some(ENGINE_GUARD) {
            run_guard(args).map_err(|error| format!("Could not guard the HoH engine ({error})."))?;
            return Ok(None);
        }
        let instance = if matches!(mode, Some(WORKER | CANDIDATE | RECOVERED)) {
            InstanceLock::inherit(args.next().as_deref())
                .map_err(|error| format!("Could not resume HoH after updating ({error})."))?
        } else {
            let lock = InstanceLock::acquire()
                .map_err(|error| format!("Could not check whether HoH is running ({error})."))?;
            let Some(lock) = lock else {
                eprintln!("HoH is already running.");
                return Ok(None);
            };
            lock
        };
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let (package, installed) = package_for(std::env::consts::OS, std::env::consts::ARCH)
            .and_then(|package| Some((package, package.installed_at(&executable)?)))
            .unzip();
        if mode == Some(WORKER) {
            let package = package.ok_or("Cannot update this installation.")?;
            let installed = installed.ok_or("Cannot find the installation to update.")?;
            let _gate = take_fd(args.next().as_deref()).map_err(|error| error.to_string())?;
            let staging = args.next().ok_or("Missing update staging directory.")?;
            let launch_args: Vec<_> = args.collect();
            worker(
                package,
                installed,
                Path::new(&staging),
                &launch_args,
                &instance,
            )
            .map_err(|error| error.to_string())?;
            return Ok(None);
        }
        let candidate = mode == Some(CANDIDATE);
        let recovered = mode == Some(RECOVERED);
        let mut installation = Self {
            instance,
            lease: None,
            job: Mutex::default(),
            readiness: None,
            recovered: Mutex::new(recovered),
        };
        let Some((package, installed)) = package.zip(installed) else {
            return Ok(Some(installation));
        };
        if is_backup(installed) {
            return Err(
                "Restore the HoH backup to its installation path before launching it.".into(),
            );
        }
        let lease_error = |error| format!("Could not lock the HoH installation ({error}).");
        let lease = if candidate || recovered {
            inherited_lease().map_err(lease_error)?
        } else {
            let lease = package.open_lease(installed).map_err(lease_error)?;
            lease.try_lock_shared().map_err(|error| match error {
                TryLockError::WouldBlock => {
                    "HoH is applying an update. Open it again when the update finishes.".into()
                }
                TryLockError::Error(error) => lease_error(error),
            })?;
            check_lease(package, installed, &lease).map_err(lease_error)?;
            if recover_pending(package, installed, &lease, &installation.instance)
                .map_err(|error| format!("Could not recover the interrupted HoH update ({error})."))?
            {
                return Ok(None);
            }
            lease
        };
        check_lease(package, installed, &lease).map_err(lease_error)?;
        let marker = read_install_metadata(&installed.join(package.version_file))
            .map_err(|error| format!("Could not read the HoH installation version ({error})."))?;
        if marker.trim() != version.to_string() {
            return Err(
                "The HoH installation has changed. Open it again to start the current version."
                    .into(),
            );
        }
        if candidate {
            let connect_error =
                |error| format!("Could not connect to the HoH update worker ({error}).");
            let lifecycle =
                UnixStream::from(take_fd(args.next().as_deref()).map_err(connect_error)?);
            lifecycle.set_write_timeout(Some(WAIT_STEP)).map_err(connect_error)?;
            let recovery = Recovery::open(installed, package)
                .map_err(connect_error)?
                .ok_or("Could not find the pending HoH update.")?;
            installation.readiness = Some(Arc::new(CandidateReadiness {
                lifecycle: Arc::new(lifecycle),
                engine_status: recovery.prepare_engine_status().map_err(connect_error)?,
                recovery,
                phase: Mutex::new(CandidatePhase::Waiting),
                ui: Mutex::default(),
            }));
        }
        installation.lease = Some(lease);
        Ok(Some(installation))
    }

    pub(super) fn can_update(&self) -> bool {
        self.lease.is_some()
    }

    pub(super) fn begin(&self) -> Result<(), String> {
        let mut job = self.job.lock().unwrap_or_else(|error| error.into_inner());
        if !matches!(*job, Job::Idle) {
            return Err("HoH is already updating.".into());
        }
        *job = Job::Installing;
        Ok(())
    }

    pub fn defer_exit(&self) -> bool {
        let mut job = self.job.lock().unwrap_or_else(|error| error.into_inner());
        if matches!(*job, Job::Installing | Job::Cancelling) {
            *job = Job::Cancelling;
            return true;
        }
        false
    }

    pub(super) fn cancelled(&self) -> bool {
        matches!(
            *self.job.lock().unwrap_or_else(|error| error.into_inner()),
            Job::Cancelling
        )
    }

    pub(super) fn finish(&self) -> bool {
        let mut job = self.job.lock().unwrap_or_else(|error| error.into_inner());
        if matches!(*job, Job::Restarting { .. }) {
            return true;
        }
        matches!(std::mem::take(&mut *job), Job::Cancelling)
    }

    pub(super) fn take_recovered(&self) -> bool {
        std::mem::take(
            &mut *self
                .recovered
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        )
    }

    pub(super) fn handoff(
        &self,
        executable: &Path,
        installed: &Path,
        staging: tempfile::TempDir,
        package: Package,
    ) -> Result<(), String> {
        let mut job = self.job.lock().unwrap_or_else(|error| error.into_inner());
        if matches!(*job, Job::Cancelling) {
            return Err("The update was cancelled.".into());
        }
        if let Some(readiness) = &self.readiness
            && *readiness.phase.lock().unwrap_or_else(|error| error.into_inner())
                != CandidatePhase::Committed
        {
            return Err("HoH is still starting after an update.".into());
        }
        let lease = self
            .lease
            .as_ref()
            .ok_or("The HoH installation is read-only.")?;
        let error_message = |error| format!("Could not prepare the HoH restart ({error}).");
        // Serialize upgrades to exclusive mode. On some systems try_lock releases the old shared lock.
        let gate = update_gate(installed).map_err(|error| {
            if error.kind() == io::ErrorKind::WouldBlock {
                "Another HoH process is applying an update.".into()
            } else {
                error_message(error)
            }
        })?;
        if Recovery::open(installed, package).map_err(error_message)?.is_some() {
            return Err(
                "An earlier HoH update is still pending. Restart HoH before trying again.".into(),
            );
        }
        if let Err(error) = lease.try_lock() {
            lease.lock_shared().map_err(error_message)?;
            return Err(match error {
                TryLockError::WouldBlock => {
                    "Close the other HoH windows from this installation, then try again.".into()
                }
                TryLockError::Error(error) => error_message(error),
            });
        }
        let result = (|| {
            check_lease(package, installed, lease)?;
            Recovery::check_exchange_permissions(
                installed,
                &package.staged_in(&staging.path().join("unpacked"))?,
            )?;
            let (until_exit, parent) = UnixStream::pair()?;
            let launch_args = current_launch_args();
            let output: OwnedFd = parent.into();
            let mut command = self.instance.restart_command(executable, WORKER)?;
            pass_fd(&mut command, &gate)?;
            let mut child = command
                .arg(staging.path())
                .args(launch_args)
                .stdin(Stdio::from(lease.try_clone()?))
                .stdout(Stdio::from(output))
                .spawn()?;
            let _ = staging.keep();
            thread::spawn(move || {
                let _ = child.wait();
            });
            *job = Job::Restarting { until_exit };
            Ok(())
        })();
        if result.is_err() {
            lease.lock_shared().map_err(error_message)?;
        }
        result.map_err(error_message)
    }

    pub fn acknowledge_shutdown(&self) {
        let job = self.job.lock().unwrap_or_else(|error| error.into_inner());
        if let Job::Restarting { until_exit } = &*job
            && let Err(error) = (&*until_exit).write_all(&[SHUTDOWN_COMPLETE])
        {
            eprintln!("HoH could not confirm shutdown to the update worker: {error}");
        }
    }

    pub fn watch_update_worker(&self, app: tauri::AppHandle) {
        if let Some(readiness) = self.readiness.clone() {
            thread::spawn(move || {
                if !monitor_worker(&readiness.lifecycle, &readiness.phase) {
                    app.exit(1);
                }
            });
        }
    }

    pub fn report_user_quit(&self) {
        if let Some(readiness) = &self.readiness {
            let mut phase = readiness.phase.lock().unwrap_or_else(|error| error.into_inner());
            if matches!(*phase, CandidatePhase::Committed | CandidatePhase::Quitting) {
                return;
            }
            *phase = CandidatePhase::Quitting;
            if let Err(error) = readiness.recovery.request_user_quit()
                && error.kind() != io::ErrorKind::AlreadyExists
            {
                eprintln!("HoH could not record the quit request: {error}");
            }
            let _ = (&*readiness.lifecycle).write_all(&USER_QUIT.to_be_bytes());
        }
    }

    pub(super) fn ui_status(&self, view: u32, ready: bool) {
        if let Some(readiness) = &self.readiness {
            let mut ui = readiness.ui.lock().unwrap_or_else(|error| error.into_inner());
            if ready {
                *ui = Some((view, Instant::now()));
            } else if ui.is_some_and(|(active, _)| active == view) {
                *ui = None;
            }
        }
    }

    pub(crate) fn ui_loading(&self) {
        if let Some(readiness) = &self.readiness {
            *readiness.ui.lock().unwrap_or_else(|error| error.into_inner()) = None;
        }
    }

    pub(crate) fn engine_ready_handler(&self) -> Option<crate::engine::ReadyHandler> {
        let readiness = self.readiness.clone()?;
        Some(Box::new(move |generation, transport| {
            let mut phase = readiness.phase.lock().unwrap_or_else(|error| error.into_inner());
            match *phase {
                CandidatePhase::Checking => {}
                CandidatePhase::Waiting | CandidatePhase::Ready => return false,
                _ => return true,
            }
            let ui = readiness.ui.lock().unwrap_or_else(|error| error.into_inner());
            let confirmed = generation.filter(|(_, engine_ready_at)| {
                ui.is_some_and(|(_, ui_ready_at)| {
                    (*engine_ready_at).max(ui_ready_at).elapsed() >= START_STABILITY
                }) && transport.confirm_running()
            });
            let reply = confirmed.map_or(0, |(generation, _)| generation).to_be_bytes();
            if let Err(error) = (&*readiness.lifecycle).write_all(&reply) {
                eprintln!("HoH could not confirm update readiness: {error}");
                return true;
            }
            *phase = if confirmed.is_some() {
                CandidatePhase::Ready
            } else {
                CandidatePhase::Waiting
            };
            false
        }))
    }

    pub(crate) fn engine_connector(&self, config: ProcessConfig) -> io::Result<Arc<dyn Connector>> {
        if let Some(readiness) = &self.readiness {
            let lease = self.lease.as_ref().ok_or(io::ErrorKind::InvalidInput)?;
            Ok(Arc::new(CandidateEngine {
                config,
                lifecycle: readiness.lifecycle.clone(),
                lease: lease.try_clone()?,
                status: readiness.engine_status.try_clone()?,
            }))
        } else {
            Ok(Arc::new(config))
        }
    }
}

fn current_launch_args() -> Vec<OsString> {
    let mut args = std::env::args_os().skip(1).peekable();
    if args.peek().is_some_and(|arg| arg == CANDIDATE) {
        args.nth(2);
    } else if args.peek().is_some_and(|arg| arg == RECOVERED) {
        args.nth(1);
    }
    args.collect()
}

fn inherited_lease() -> io::Result<File> {
    let input = std::io::stdin();
    let lease = File::from(input.as_fd().try_clone_to_owned()?);
    // WebKit and other children must not keep the installation lock after HoH quits.
    if unsafe { libc::fcntl(input.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(lease)
}

fn launch(
    instance: &InstanceLock,
    executable: &Path,
    mode: &str,
    lifecycle: Option<&UnixStream>,
    args: &[OsString],
    lease: &File,
) -> io::Result<Child> {
    let mut command = instance.restart_command(executable, mode)?;
    if let Some(lifecycle) = lifecycle {
        pass_fd(&mut command, lifecycle)?;
    }
    command
        .args(args)
        .stdin(Stdio::from(lease.try_clone()?))
        .stdout(Stdio::null())
        .spawn()
}

enum StartupOutcome {
    Ready,
    Failed,
    UserQuit,
}

fn await_ready(child: &mut Child, lifecycle: &mut UnixStream) -> io::Result<StartupOutcome> {
    let deadline = Instant::now() + START_TIMEOUT;
    lifecycle.set_read_timeout(Some(WAIT_STEP))?;
    lifecycle.set_write_timeout(Some(WAIT_STEP))?;
    let mut reply = [0; 8];
    let mut received = 0;
    lifecycle.write_all(&[CHECK_READY])?;
    while Instant::now() < deadline {
        match lifecycle.read(&mut reply[received..]) {
            Ok(0) => break,
            Ok(count) => received += count,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error),
        }
        if received == reply.len() && u64::from_be_bytes(reply) == USER_QUIT {
            return Ok(StartupOutcome::UserQuit);
        }
        if child.try_wait()?.is_some() {
            break;
        }
        if received == reply.len() {
            if u64::from_be_bytes(reply) != 0 {
                return Ok(StartupOutcome::Ready);
            }
            thread::sleep(WAIT_STEP);
            lifecycle.write_all(&[CHECK_READY])?;
            received = 0;
        }
    }
    Ok(StartupOutcome::Failed)
}

fn stop_candidate(
    child: &mut Child,
    lifecycle: &mut UnixStream,
    recovery: &Recovery,
) -> io::Result<()> {
    let _ = lifecycle.shutdown(Shutdown::Write);
    let deadline = Instant::now() + STOP_TIMEOUT;
    while child.try_wait()?.is_none() && Instant::now() < deadline {
        thread::sleep(WAIT_STEP);
    }
    if child.try_wait()?.is_none()
        && let Err(error) = child.kill()
        && child.try_wait()?.is_none()
    {
        return Err(error);
    }
    let deadline = Instant::now() + STOP_TIMEOUT;
    while child.try_wait()?.is_none() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the updated HoH did not stop",
            ));
        }
        thread::sleep(WAIT_STEP);
    }
    await_candidate_socket_close(lifecycle, deadline)?;
    if let Some(status) = recovery.engine_status()? {
        await_engine_stop(&status, deadline)?;
    }
    Ok(())
}

fn await_candidate_socket_close(lifecycle: &mut UnixStream, deadline: Instant) -> io::Result<()> {
    lifecycle.set_nonblocking(true)?;
    while Instant::now() < deadline {
        match lifecycle.read(&mut [0; 64]) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(WAIT_STEP),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::ConnectionReset
                ) => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "the updated HoH lifecycle socket did not close",
    ))
}

fn recover_pending(
    package: Package,
    installed: &Path,
    active: &File,
    instance: &InstanceLock,
) -> io::Result<bool> {
    let Some(recovery) = Recovery::open(installed, package)? else {
        return Ok(false);
    };
    let _gate = update_gate(installed)?;
    active.try_lock()?;
    check_lease(package, installed, active)?;
    let backup = package.open_lease(recovery.backup())?;
    backup.try_lock()?;
    check_lease(package, recovery.backup(), &backup)?;
    #[cfg(target_os = "linux")]
    let legacy_leases = if recovery.marker_lease() {
        None
    } else {
        let leases = [File::open(installed)?, File::open(recovery.backup())?];
        for lease in &leases {
            lease.try_lock()?;
        }
        Some(leases)
    };
    let swapped = recovery.is_swapped(installed)?;
    let restored = swapped && recovery.decision()? != Decision::Committed;
    if swapped && let Some(status) = recovery.engine_status()? {
        confirm_interrupted_engine_stopped(&status)?;
    }
    if restored {
        recovery.exchange(installed)?;
    }
    recovery.sync_exchange(installed)?;
    if let Err(error) = recovery.finish() {
        eprintln!("HoH could not clean up the interrupted update: {error}");
    }
    if restored {
        let lease = &backup;
        #[cfg(target_os = "linux")]
        let lease = legacy_leases.as_ref().map_or(lease, |leases| &leases[1]);
        restart_previous(package, installed, &current_launch_args(), instance, lease)?;
    } else {
        active.lock_shared()?;
    }
    Ok(restored)
}

fn restart_previous(
    package: Package,
    installed: &Path,
    args: &[OsString],
    instance: &InstanceLock,
    lease: &File,
) -> io::Result<()> {
    let executable = installed.join(package.binary_dir).join("hoh-desktop");
    lease.lock_shared()?;
    launch(instance, &executable, RECOVERED, None, args, lease)?;
    Ok(())
}

fn worker(
    package: Package,
    installed: &Path,
    staging: &Path,
    args: &[OsString],
    instance: &InstanceLock,
) -> io::Result<()> {
    if staging.parent() != installed.parent()
        || !staging
            .file_name()
            .is_some_and(|name| name.as_bytes().starts_with(b".hoh-update-"))
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let lease = inherited_lease()?;
    check_lease(package, installed, &lease)?;
    let mut parent = UnixStream::from(std::io::stdout().as_fd().try_clone_to_owned()?);
    if let Err(error) = await_parent_exit(&mut parent) {
        let _ = fs::remove_dir_all(staging);
        return Err(error);
    }
    let (recovery, candidate) = match Recovery::publish(installed, staging, package) {
        Ok(prepared) => prepared,
        Err(error) => {
            eprintln!("HoH could not prepare the update recovery directory: {error}");
            let _ = fs::remove_dir_all(staging);
            return restart_previous(package, installed, args, instance, &lease);
        }
    };
    let executable = installed.join(package.binary_dir).join("hoh-desktop");
    if let Err(error) = recovery.exchange(installed) {
        eprintln!("HoH could not replace the installation: {error}");
        let _ = recovery.finish();
        return restart_previous(package, installed, args, instance, &lease);
    }
    let mut user_quit = false;
    if recovery.sync_exchange(installed).is_ok()
        && let Ok((mut lifecycle, candidate_end)) = UnixStream::pair()
        && let Ok(mut child) = launch(
            instance,
            &executable,
            CANDIDATE,
            Some(&candidate_end),
            args,
            &candidate,
        )
    {
        drop(candidate_end);
        match await_ready(&mut child, &mut lifecycle) {
            Ok(StartupOutcome::Ready) => match recovery.commit() {
                Ok(()) => {
                    candidate.lock_shared()?;
                    lifecycle.write_all(&[COMMIT_COMPLETE])?;
                    return recovery.finish();
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => eprintln!("HoH could not commit the updated installation: {error}"),
            },
            Ok(StartupOutcome::UserQuit) => user_quit = true,
            Ok(StartupOutcome::Failed) => {}
            Err(error) => eprintln!("HoH could not confirm the updated installation: {error}"),
        }
        stop_candidate(&mut child, &mut lifecycle, &recovery)?;
    }
    match recovery.decision()? {
        Decision::Committed => {
            return Err(io::Error::other("the update commit could not be confirmed"));
        }
        Decision::UserQuit => user_quit = true,
        Decision::Pending | Decision::Rollback => {}
    }
    recovery.exchange(installed)?;
    let result = recovery.sync_exchange(installed).and_then(|()| recovery.finish());
    if !user_quit {
        restart_previous(package, installed, args, instance, &lease)?;
    }
    result
}

fn await_parent_exit(parent: &mut UnixStream) -> io::Result<()> {
    parent.set_read_timeout(Some(START_TIMEOUT))?;
    let mut reply = [0];
    if parent.read(&mut reply)? == 1 && reply[0] == SHUTDOWN_COMPLETE {
        parent.set_read_timeout(Some(STOP_TIMEOUT))?;
        if parent.read(&mut reply)? == 0 {
            return Ok(());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "could not confirm the previous HoH shut down",
    ))
}

pub(super) enum ChildWaitError {
    Interrupted,
    ShutdownUnconfirmed(io::Error),
}

pub(super) fn wait_for_child_group(
    child: &mut Child,
    stopped: impl Fn() -> bool,
) -> Result<ExitStatus, ChildWaitError> {
    loop {
        if stopped() {
            stop_child_group(child).map_err(ChildWaitError::ShutdownUnconfirmed)?;
            return Err(ChildWaitError::Interrupted);
        }
        if let Some(status) = child
            .try_wait()
            .map_err(ChildWaitError::ShutdownUnconfirmed)?
        {
            confirm_child_group_stopped(child).map_err(ChildWaitError::ShutdownUnconfirmed)?;
            return Ok(status);
        }
        thread::sleep(WAIT_STEP);
    }
}

pub(super) fn stop_child_group(child: &mut Child) -> io::Result<()> {
    if unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) } < 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error);
        }
    }
    confirm_child_group_stopped(child)
}

fn confirm_child_group_stopped(child: &mut Child) -> io::Result<()> {
    let deadline = Instant::now() + HELPER_STOP_TIMEOUT;
    let group = -(child.id() as libc::pid_t);
    loop {
        if child.try_wait()?.is_some() && unsafe { libc::kill(group, 0) } < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(());
            }
            return Err(error);
        }
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        thread::sleep(WAIT_STEP);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_loss_after_readiness_requires_shutdown() {
        let (worker, candidate) = UnixStream::pair().unwrap();
        let phase = Mutex::new(CandidatePhase::Ready);
        drop(worker);

        assert!(!monitor_worker(&candidate, &phase));
        assert!(*phase.lock().unwrap() == CandidatePhase::Aborted);
    }

    #[test]
    fn commit_acknowledgement_allows_candidate_to_keep_running() {
        let (mut worker, candidate) = UnixStream::pair().unwrap();
        let phase = Mutex::new(CandidatePhase::Ready);
        worker.write_all(&[COMMIT_COMPLETE]).unwrap();
        drop(worker);

        assert!(monitor_worker(&candidate, &phase));
        assert!(*phase.lock().unwrap() == CandidatePhase::Committed);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn search_only_bundle_uses_the_readable_marker_lease() {
        let package = package_for("linux", "x86_64").unwrap();
        let parent = tempfile::tempdir().unwrap();
        let bundle = parent.path().join(package.install_dir);
        fs::create_dir(&bundle).unwrap();
        fs::write(bundle.join(package.version_file), b"0.0.1\n").unwrap();
        fs::set_permissions(&bundle, fs::Permissions::from_mode(0o111)).unwrap();

        let active = package.open_lease(&bundle).unwrap();
        let updater = package.open_lease(&bundle).unwrap();
        fs::set_permissions(&bundle, fs::Permissions::from_mode(0o755)).unwrap();
        active.try_lock_shared().unwrap();
        check_lease(package, &bundle, &active).unwrap();
        assert!(matches!(updater.try_lock(), Err(TryLockError::WouldBlock)));
        drop(active);
        updater.try_lock().unwrap();
        assert!(matches!(
            package.open_lease(&bundle).unwrap().try_lock_shared(),
            Err(TryLockError::WouldBlock)
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn candidate_socket_drains_to_eof_after_reset() {
        let (mut worker, candidate) = UnixStream::pair().unwrap();
        worker.write_all(&[CHECK_READY]).unwrap();
        drop(candidate);

        await_candidate_socket_close(&mut worker, Instant::now() + STOP_TIMEOUT).unwrap();
    }
}
