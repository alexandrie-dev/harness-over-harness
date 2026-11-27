use super::{
    Package,
    candidate::{CandidateEngine, ENGINE_GUARD, run_guard},
    instance::{InstanceLock, pass_fd, take_fd},
    open_regular_file, package_for, read_install_metadata,
    recovery::{Recovery, is_backup, sibling_path},
};
use crate::engine::{Connector, ProcessConfig};
use semver::Version;
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, TryLockError},
    io::{self, Read, Write},
    os::{
        fd::{AsFd, AsRawFd, OwnedFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, PermissionsExt},
            net::UnixStream,
        },
    },
    path::Path,
    process::{Child, Stdio},
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
const WAIT_STEP: Duration = Duration::from_millis(75);

#[derive(Default)]
enum Job {
    #[default]
    Idle,
    Installing,
    Cancelling,
    Restarting {
        _until_exit: UnixStream,
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
    ui: Mutex<Option<(u32, Instant)>>,
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

fn check_lease(installed: &Path, lease: &File) -> io::Result<()> {
    let active = fs::metadata(installed)?;
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
        if candidate {
            let connect_error =
                |error| format!("Could not connect to the HoH update worker ({error}).");
            let lifecycle =
                UnixStream::from(take_fd(args.next().as_deref()).map_err(connect_error)?);
            lifecycle.set_nonblocking(true).map_err(connect_error)?;
            installation.readiness = Some(Arc::new(CandidateReadiness {
                lifecycle: Arc::new(lifecycle),
                ui: Mutex::default(),
            }));
        }
        let lease_error = |error| format!("Could not lock the HoH installation ({error}).");
        let lease = if candidate || recovered {
            inherited_lease().map_err(lease_error)?
        } else {
            let lease = File::open(installed).map_err(lease_error)?;
            lease.try_lock_shared().map_err(|error| match error {
                TryLockError::WouldBlock => {
                    "HoH is applying an update. Open it again when the update finishes.".into()
                }
                TryLockError::Error(error) => lease_error(error),
            })?;
            check_lease(installed, &lease).map_err(lease_error)?;
            if recover_pending(package, installed, &lease, &installation.instance)
                .map_err(|error| format!("Could not recover the interrupted HoH update ({error})."))?
            {
                return Ok(None);
            }
            lease
        };
        check_lease(installed, &lease).map_err(lease_error)?;
        let marker = read_install_metadata(&installed.join(package.version_file))
            .map_err(|error| format!("Could not read the HoH installation version ({error})."))?;
        if marker.trim() != version.to_string() {
            return Err(
                "The HoH installation has changed. Open it again to start the current version."
                    .into(),
            );
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
    ) -> Result<(), String> {
        let mut job = self.job.lock().unwrap_or_else(|error| error.into_inner());
        if matches!(*job, Job::Cancelling) {
            return Err("The update was cancelled.".into());
        }
        let lease = self
            .lease
            .as_ref()
            .ok_or("The HoH installation is read-only.")?;
        let error_message = |error| format!("Could not prepare the HoH restart ({error}).");
        // Serialize upgrades to exclusive mode. On some systems try_lock releases the old shared lock.
        let _gate = update_gate(installed).map_err(|error| {
            if error.kind() == io::ErrorKind::WouldBlock {
                "Another HoH process is applying an update.".into()
            } else {
                error_message(error)
            }
        })?;
        if Recovery::open(installed).map_err(error_message)?.is_some() {
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
            let (until_exit, parent) = UnixStream::pair()?;
            let launch_args = current_launch_args();
            let output: OwnedFd = parent.into();
            let mut child = self
                .instance
                .restart_command(executable, WORKER)?
                .arg(staging.path())
                .args(launch_args)
                .stdin(Stdio::from(lease.try_clone()?))
                .stdout(Stdio::from(output))
                .spawn()?;
            let _ = staging.keep();
            thread::spawn(move || {
                let _ = child.wait();
            });
            *job = Job::Restarting {
                _until_exit: until_exit,
            };
            Ok(())
        })();
        if result.is_err() {
            lease.lock_shared().map_err(error_message)?;
        }
        result.map_err(error_message)
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
        let mut reported = false;
        Some(Box::new(move |engine_ready_at| {
            let mut lifecycle = &*readiness.lifecycle;
            match lifecycle.read(&mut [0]) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) if error.kind() == io::ErrorKind::Interrupted => return false,
                _ => return true,
            }
            let ready = engine_ready_at.is_some_and(|engine_ready_at| {
                let ui = readiness.ui.lock().unwrap_or_else(|error| error.into_inner());
                ui.is_some_and(|(_, ui_ready_at)| {
                    engine_ready_at.max(ui_ready_at).elapsed() >= START_STABILITY
                })
            });
            if ready != reported {
                match lifecycle.write_all(&[u8::from(ready)]) {
                    Ok(()) => reported = ready,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => {
                        eprintln!("HoH could not report update readiness: {error}");
                        return true;
                    }
                }
            }
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

fn await_ready(child: &mut Child, lifecycle: &mut UnixStream) -> io::Result<bool> {
    let deadline = Instant::now() + START_TIMEOUT;
    lifecycle.set_nonblocking(true)?;
    let mut changes = [0; 64];
    loop {
        let mut ready = false;
        loop {
            match lifecycle.read(&mut changes) {
                Ok(0) => break,
                Ok(count) => ready = changes[count - 1] == 1,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        let exit = child.try_wait()?;
        if exit.is_some_and(|status| !status.success()) {
            return Ok(false);
        }
        if ready {
            return Ok(true);
        }
        if exit.is_some() || Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(WAIT_STEP);
    }
}

fn stop_candidate(child: &mut Child, lifecycle: &mut UnixStream) -> io::Result<()> {
    if let Err(error) = child.kill()
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
    lifecycle.set_nonblocking(false)?;
    lifecycle.set_read_timeout(Some(STOP_TIMEOUT))?;
    io::copy(lifecycle, &mut io::sink())?;
    Ok(())
}

fn recover_pending(
    package: Package,
    installed: &Path,
    active: &File,
    instance: &InstanceLock,
) -> io::Result<bool> {
    let Some(recovery) = Recovery::open(installed)? else {
        return Ok(false);
    };
    let _gate = update_gate(installed)?;
    active.try_lock()?;
    check_lease(installed, active)?;
    let backup = File::open(recovery.backup(package))?;
    backup.try_lock()?;
    let restored = recovery.is_swapped(active, &backup)? && !recovery.committed()?;
    if restored {
        recovery.exchange(installed, package)?;
    }
    recovery.sync_exchange(installed)?;
    if let Err(error) = recovery.finish() {
        eprintln!("HoH could not clean up the interrupted update: {error}");
    }
    if restored {
        restart_previous(package, installed, &current_launch_args(), instance, &backup)?;
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
    check_lease(installed, &lease)?;
    // This socket closes only after the previous process has shut down its engine and exited.
    let mut parent = UnixStream::from(std::io::stdout().as_fd().try_clone_to_owned()?);
    parent.set_read_timeout(Some(START_TIMEOUT))?;
    match parent.read(&mut [0]) {
        Ok(0) => {}
        result => {
            let _ = fs::remove_dir_all(staging);
            return Err(result
                .err()
                .unwrap_or_else(|| io::ErrorKind::InvalidData.into()));
        }
    }
    let (recovery, candidate) = match Recovery::publish(installed, staging, package, &lease) {
        Ok(prepared) => prepared,
        Err(error) => {
            eprintln!("HoH could not prepare the update recovery directory: {error}");
            let _ = fs::remove_dir_all(staging);
            return restart_previous(package, installed, args, instance, &lease);
        }
    };
    let executable = installed.join(package.binary_dir).join("hoh-desktop");
    if let Err(error) = recovery.exchange(installed, package) {
        eprintln!("HoH could not replace the installation: {error}");
        let _ = recovery.finish();
        return restart_previous(package, installed, args, instance, &lease);
    }
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
        if await_ready(&mut child, &mut lifecycle).unwrap_or(false) {
            match recovery.commit() {
                Ok(()) => {
                    let result = recovery.finish();
                    candidate.lock_shared()?;
                    return result;
                }
                Err(error) => eprintln!("HoH could not commit the updated installation: {error}"),
            }
        }
        stop_candidate(&mut child, &mut lifecycle)?;
    }
    recovery.exchange(installed, package)?;
    let result = recovery.sync_exchange(installed).and_then(|()| recovery.finish());
    restart_previous(package, installed, args, instance, &lease)?;
    result
}

pub(super) fn wait_for_child(
    child: &mut Child,
    cancelled: impl Fn() -> bool,
) -> io::Result<std::process::ExitStatus> {
    wait_for(child, cancelled, |child| {
        let _ = child.kill();
    })
}

pub(super) fn wait_for_group(
    child: &mut Child,
    cancelled: impl Fn() -> bool,
) -> io::Result<std::process::ExitStatus> {
    wait_for(child, cancelled, |child| {
        // The extractor starts its own process group, including its decompressor.
        unsafe {
            libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
        }
    })
}

fn wait_for(
    child: &mut Child,
    cancelled: impl Fn() -> bool,
    stop: impl Fn(&mut Child),
) -> io::Result<std::process::ExitStatus> {
    loop {
        if cancelled() {
            stop(child);
            child.wait()?;
            return Err(io::ErrorKind::Interrupted.into());
        }
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        thread::sleep(WAIT_STEP);
    }
}
