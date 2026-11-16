use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const LAUNCHER_BIN: &str = "landlock-run";
pub const LAUNCHER_FAILURE_EXIT: u8 = 125;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LandlockEnforcement {
    Full,
    Partial,
    Unusable,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LauncherGrants {
    pub read_only: Vec<PathBuf>,
    pub read_write: Vec<PathBuf>,
}

/// Read-only roots first, preserving duplicates and caller ordering.
pub fn grant_args(grants: &LauncherGrants) -> Vec<OsString> {
    let mut args = Vec::new();
    for (flag, paths) in [("--ro", &grants.read_only), ("--rw", &grants.read_write)] {
        for path in paths {
            args.push(flag.into());
            args.push(path.as_os_str().to_owned());
        }
    }
    args
}

/// The launcher beside this executable; existence is deliberately not checked.
pub fn launcher_path() -> io::Result<PathBuf> {
    Ok(std::env::current_exe()?.with_file_name(LAUNCHER_BIN))
}

/// Synchronous functional probe. `None` selects the sibling launcher / 2000 ms.
/// A zero timeout disables the deadline, as in spawnSync. A successful exit
/// without the substring `partially enforced` means full enforcement.
pub fn probe(launcher: Option<&Path>, timeout: Option<Duration>) -> LandlockEnforcement {
    #[cfg(unix)]
    {
        let default_path;
        let launcher = match launcher {
            Some(path) => path,
            None => {
                default_path = match launcher_path() {
                    Ok(path) => path,
                    Err(_) => return LandlockEnforcement::Unusable,
                };
                &default_path
            }
        };
        probe_posix(launcher, timeout.unwrap_or(Duration::from_millis(2000)))
            .unwrap_or(LandlockEnforcement::Unusable)
    }
    #[cfg(not(unix))]
    {
        let _ = (launcher, timeout);
        LandlockEnforcement::Unusable
    }
}

#[cfg(unix)]
fn probe_posix(launcher: &Path, timeout: Duration) -> io::Result<LandlockEnforcement> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    let mut child = Command::new(launcher)
        .arg("--probe")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let result = (|| {
        let mut pipe = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("missing probe stdout"))?;
        let flags = rustix::fs::fcntl_getfl(&pipe)?;
        rustix::fs::fcntl_setfl(&pipe, flags | rustix::fs::OFlags::NONBLOCK)?;
        let start = Instant::now();
        let mut output = Vec::new();
        let mut status = None;
        loop {
            let mut eof = false;
            let mut buffer = [0; 8192];
            loop {
                match pipe.read(&mut buffer) {
                    Ok(0) => {
                        eof = true;
                        break;
                    }
                    Ok(count) => {
                        output.extend_from_slice(&buffer[..count]);
                        // Node spawnSync's default maxBuffer is one MiB.
                        if output.len() > 1024 * 1024 {
                            return Ok(LandlockEnforcement::Unusable);
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error),
                }
            }
            if status.is_none() {
                status = child.try_wait()?;
            }
            if eof && let Some(status) = status {
                return Ok(if !status.success() {
                    LandlockEnforcement::Unusable
                } else if output
                    .windows(b"partially enforced".len())
                    .any(|s| s == b"partially enforced")
                {
                    LandlockEnforcement::Partial
                } else {
                    LandlockEnforcement::Full
                });
            }
            if !timeout.is_zero() && start.elapsed() >= timeout {
                return Ok(LandlockEnforcement::Unusable);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    })();
    // spawnSync uses SIGTERM by default. Do not signal an already reaped PID.
    match child.try_wait() {
        Ok(None) => {
            let _ = rustix::process::kill_process(
                rustix::process::Pid::from_child(&child),
                rustix::process::Signal::TERM,
            );
        }
        Ok(Some(_)) => {}
        Err(_) => {
            let _ = child.kill();
        }
    }
    let _ = child.wait();
    result
}
