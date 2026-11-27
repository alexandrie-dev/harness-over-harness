use super::{Package, package_for, swap_directories};
use semver::Version;
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read},
    os::{
        fd::{AsFd, AsRawFd, OwnedFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tauri::Manager;

const WORKER: &str = "--hoh-update-worker";
const CANDIDATE: &str = "--hoh-update-candidate";
const RECOVERED: &str = "--hoh-update-recovered";
const START_TIMEOUT: Duration = Duration::from_secs(45);
const READY_FILE: &str = "ready";
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

#[derive(Default)]
pub struct Installation {
    lease: Option<File>,
    job: Mutex<Job>,
    ready: Option<PathBuf>,
    engine_ready: AtomicBool,
    page_loaded: AtomicBool,
    recovered: Mutex<bool>,
}

fn read_lock(installed: &Path, kind: &str) -> io::Result<File> {
    use sha2::{Digest, Sha256};
    let name = installed.file_name().ok_or(io::ErrorKind::InvalidInput)?;
    let key = Sha256::digest(name.as_bytes());
    let key = hoh_protocol::approval::hex(&key[..12]);
    let parent = installed.parent().ok_or(io::ErrorKind::InvalidInput)?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(parent.join(format!(".hoh-{key}-{kind}.lock")))
}

impl Installation {
    pub fn startup(version: &Version) -> Result<Option<Self>, String> {
        let mut args = std::env::args_os().skip(1);
        let mode = args.next();
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let (package, installed) = package_for(std::env::consts::OS, std::env::consts::ARCH)
            .and_then(|package| Some((package, package.installed_at(&executable)?)))
            .unzip();
        if mode.as_deref().is_some_and(|mode| mode == WORKER) {
            let package = package.ok_or("Cannot update this installation.")?;
            let installed = installed.ok_or("Cannot find the installation to update.")?;
            let staging = args.next().ok_or("Missing update staging directory.")?;
            let launch_args: Vec<_> = args.collect();
            worker(package, installed, Path::new(&staging), &launch_args)
                .map_err(|error| error.to_string())?;
            return Ok(None);
        }
        let Some((package, installed)) = package.zip(installed) else {
            return Ok(Some(Self::default()));
        };
        let candidate = mode.as_deref().is_some_and(|mode| mode == CANDIDATE);
        let recovered = mode.as_deref().is_some_and(|mode| mode == RECOVERED);
        let ready = if candidate {
            Some(PathBuf::from(
                args.next().ok_or("Missing update readiness path.")?,
            ))
        } else {
            None
        };
        let lease = if candidate || recovered {
            Some(inherited_lease().map_err(|error| error.to_string())?)
        } else if let Ok(lease) = read_lock(installed, "running") {
            lease.try_lock_shared().map_err(|error| {
                format!(
                    "HoH is applying an update. Open it again when the update finishes ({error})."
                )
            })?;
            Some(lease)
        } else {
            None
        };
        let marker = fs::read_to_string(installed.join(package.version_file))
            .map_err(|error| format!("Could not read the HoH installation version ({error})."))?;
        if marker.trim() != version.to_string() {
            return Err(
                "The HoH installation has changed. Open it again to start the current version."
                    .into(),
            );
        }
        Ok(Some(Self {
            lease,
            ready,
            recovered: Mutex::new(recovered),
            ..Self::default()
        }))
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
        let gate = read_lock(installed, "update").map_err(error_message)?;
        gate.try_lock()
            .map_err(|_| "Another HoH process is applying an update.")?;
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
            let mut child = Command::new(executable)
                .arg(WORKER)
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

    pub fn engine_ready(app: &tauri::AppHandle) {
        let installation = app.state::<Self>();
        installation.engine_ready.store(true, Ordering::SeqCst);
        installation.mark_ready();
    }

    pub fn page_loaded(&self) {
        self.page_loaded.store(true, Ordering::SeqCst);
        self.mark_ready();
    }

    fn mark_ready(&self) {
        if self.engine_ready.load(Ordering::SeqCst)
            && self.page_loaded.load(Ordering::SeqCst)
            && let Some(ready) = self.ready.as_deref()
        {
            let _ = File::create_new(ready);
        }
    }
}

fn current_launch_args() -> Vec<OsString> {
    let mut args = std::env::args_os().skip(1).peekable();
    if args.peek().is_some_and(|arg| arg == CANDIDATE) {
        args.next();
        args.next();
    } else if args.peek().is_some_and(|arg| arg == RECOVERED) {
        args.next();
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
    executable: &Path,
    mode: &str,
    ready: Option<&Path>,
    args: &[OsString],
    lease: &File,
) -> io::Result<Child> {
    Command::new(executable)
        .arg(mode)
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

fn worker(package: Package, installed: &Path, staging: &Path, args: &[OsString]) -> io::Result<()> {
    if staging.parent() != installed.parent()
        || !staging
            .file_name()
            .is_some_and(|name| name.as_bytes().starts_with(b".hoh-update-"))
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let lease = inherited_lease()?;
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
    let staged = package.staged_in(&staging.join("unpacked"));
    let ready = staging.join(READY_FILE);
    let executable = installed.join(package.binary_dir).join("hoh-desktop");
    if let Err(error) = swap_directories(installed, &staged) {
        eprintln!("HoH could not replace the installation: {error}");
        lease.lock_shared()?;
        launch(&executable, RECOVERED, None, args, &lease)?;
        return fs::remove_dir_all(staging);
    }
    if let Ok(mut child) = launch(&executable, CANDIDATE, Some(&ready), args, &lease) {
        if await_ready(&mut child, &ready).unwrap_or(false) {
            lease.lock_shared()?;
            return fs::remove_dir_all(staging);
        }
        let _ = child.kill();
        child.wait()?;
    }
    // Keep the staging directory if the reverse swap fails. It contains the previous copy.
    swap_directories(installed, &staged)?;
    lease.lock_shared()?;
    launch(&executable, RECOVERED, None, args, &lease)?;
    fs::remove_dir_all(staging)
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
