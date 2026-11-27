use super::{
    Package, instance::InstanceLock, package_for,
    recovery::{Recovery, is_backup, sibling_path},
};
use semver::Version;
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{self, Read},
    os::{
        fd::{AsFd, AsRawFd, OwnedFd},
        unix::{ffi::OsStrExt, fs::MetadataExt, net::UnixStream},
    },
    path::{Path, PathBuf},
    process::{Child, Stdio},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};
use tauri::Manager;

const WORKER: &str = "--hoh-update-worker";
const CANDIDATE: &str = "--hoh-update-candidate";
const RECOVERED: &str = "--hoh-update-recovered";
const START_TIMEOUT: Duration = Duration::from_secs(45);
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
    readiness: Option<Mutex<CandidateReadiness>>,
    recovered: Mutex<bool>,
}

struct CandidateReadiness {
    marker: PathBuf,
    engine: bool,
    ui: bool,
}

fn update_gate(installed: &Path) -> io::Result<File> {
    let gate = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(sibling_path(installed, "-update.lock")?)?;
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
            installation.readiness = Some(Mutex::new(CandidateReadiness {
                marker: PathBuf::from(args.next().ok_or("Missing update readiness path.")?),
                engine: false,
                ui: false,
            }));
        }
        let lease_error = |error| format!("Could not lock the HoH installation ({error}).");
        let lease = if candidate || recovered {
            inherited_lease().map_err(lease_error)?
        } else {
            let lease = File::open(installed).map_err(lease_error)?;
            lease.try_lock_shared().map_err(|error| {
                format!("HoH is applying an update. Open it again when the update finishes ({error}).")
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
        let marker = fs::read_to_string(installed.join(package.version_file))
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
        let _gate = update_gate(installed)
            .map_err(|_| "Another HoH process is applying an update.")?;
        if Recovery::open(installed).map_err(error_message)?.is_some() {
            return Err(
                "An earlier HoH update is still pending. Restart HoH before trying again.".into(),
            );
        }
        if lease.try_lock().is_err() {
            lease.lock_shared().map_err(error_message)?;
            return Err(
                "Close the other HoH windows from this installation, then try again.".into(),
            );
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

    pub fn engine_status(app: &tauri::AppHandle, ready: bool) {
        app.state::<Self>().report_readiness(|state| state.engine = ready);
    }

    pub(super) fn ui_ready(&self) {
        self.report_readiness(|state| state.ui = true);
    }

    fn report_readiness(&self, update: impl FnOnce(&mut CandidateReadiness)) {
        if let Some(readiness) = &self.readiness {
            let mut state = readiness.lock().unwrap_or_else(|error| error.into_inner());
            update(&mut state);
            if state.engine && state.ui {
                let _ = File::create_new(&state.marker);
            }
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
    ready: Option<&Path>,
    args: &[OsString],
    lease: &File,
) -> io::Result<Child> {
    instance
        .restart_command(executable, mode)?
        .args(ready)
        .args(args)
        .stdin(Stdio::from(lease.try_clone()?))
        .stdout(Stdio::null())
        .spawn()
}

fn await_ready(child: &mut Child, ready: &Path) -> io::Result<bool> {
    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        if child.try_wait()?.is_some() {
            return Ok(false);
        }
        if ready.is_file() {
            return Ok(true);
        }
        thread::sleep(WAIT_STEP);
    }
    Ok(false)
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
    let restored = recovery.is_swapped(active, &backup)?;
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
    let ready = recovery.ready_path();
    let executable = installed.join(package.binary_dir).join("hoh-desktop");
    if let Err(error) = recovery.exchange(installed, package) {
        eprintln!("HoH could not replace the installation: {error}");
        let _ = recovery.finish();
        return restart_previous(package, installed, args, instance, &lease);
    }
    if recovery.sync_exchange(installed).is_ok()
        && let Ok(mut child) =
            launch(instance, &executable, CANDIDATE, Some(&ready), args, &candidate)
    {
        if await_ready(&mut child, &ready).unwrap_or(false) {
            let result = recovery.finish();
            candidate.lock_shared()?;
            return result;
        }
        let _ = child.kill();
        child.wait()?;
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
