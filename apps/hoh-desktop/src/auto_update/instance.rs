use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions, TryLockError},
    io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::{
            fs::{MetadataExt, OpenOptionsExt},
            process::CommandExt,
        },
    },
    path::Path,
    process::Command,
};

const FIRST_NON_STDIO_FD: RawFd = 3;

pub(super) struct InstanceLock(File);

fn open_lock() -> io::Result<File> {
    let home = std::env::home_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "home directory unavailable"))?;
    #[cfg(target_os = "linux")]
    let directory = home.join(".local/state/local.hoh.desktop");
    #[cfg(target_os = "macos")]
    let directory = home.join("Library/Application Support/local.hoh.desktop");
    fs::create_dir_all(&directory)?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(directory.join("desktop.lock"))
}

impl InstanceLock {
    pub(super) fn acquire() -> io::Result<Option<Self>> {
        let file = open_lock()?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self(file))),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn inherit(argument: Option<&OsStr>) -> io::Result<Self> {
        let descriptor = argument
            .and_then(OsStr::to_str)
            .and_then(|value| value.parse::<RawFd>().ok())
            .filter(|descriptor| *descriptor >= FIRST_NON_STDIO_FD)
            .ok_or(io::ErrorKind::InvalidInput)?;
        if unsafe { libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // The restarting process owns this descriptor, inherited specifically for the lock.
        let file = unsafe { File::from_raw_fd(descriptor) };
        let expected = open_lock()?.metadata()?;
        let inherited = file.metadata()?;
        if (inherited.dev(), inherited.ino()) != (expected.dev(), expected.ino()) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        file.try_lock()?;
        Ok(Self(file))
    }

    pub(super) fn restart_command(&self, executable: &Path, mode: &str) -> io::Result<Command> {
        let descriptor = unsafe {
            libc::fcntl(
                self.0.as_raw_fd(),
                libc::F_DUPFD_CLOEXEC,
                FIRST_NON_STDIO_FD,
            )
        };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        let inherited = unsafe { OwnedFd::from_raw_fd(descriptor) };
        let mut command = Command::new(executable);
        command.arg(mode).arg(descriptor.to_string());
        // Keep the lock across this exec only. The new process restores close-on-exec.
        unsafe {
            command.pre_exec(move || {
                if libc::fcntl(inherited.as_raw_fd(), libc::F_SETFD, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(command)
    }
}
