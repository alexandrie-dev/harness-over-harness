/// One nonblocking exclusive flock attempt, borrowing the caller's descriptor.
/// Errno is returned unchanged (in particular AGAIN/WOULDBLOCK on contention).
#[cfg(unix)]
pub fn try_lock_exclusive(fd: impl std::os::fd::AsFd) -> std::io::Result<()> {
    #[cfg(all(
        any(target_os = "linux", target_os = "macos"),
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    {
        rustix::fs::flock(fd, rustix::fs::FlockOperation::NonBlockingLockExclusive)
            .map_err(Into::into)
    }
    #[cfg(not(all(
        any(target_os = "linux", target_os = "macos"),
        any(target_arch = "x86_64", target_arch = "aarch64")
    )))]
    {
        let _ = fd;
        Err(unsupported_platform())
    }
}

/// No POSIX addon exists on Windows.
#[cfg(not(unix))]
pub fn try_lock_exclusive(_fd: &std::fs::File) -> std::io::Result<()> {
    Err(unsupported_platform())
}

#[cfg(not(all(
    any(target_os = "linux", target_os = "macos"),
    any(target_arch = "x86_64", target_arch = "aarch64")
)))]
fn unsupported_platform() -> std::io::Error {
    let platform = match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "ia32",
        other => other,
    };
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        format!("ERR_FLOCK_UNSUPPORTED_PLATFORM: flock is not supported on {platform}-{arch}"),
    )
}
