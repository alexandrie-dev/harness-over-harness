use super::install::wait_for_group;
use std::{
    fs::File,
    io::{self, Read},
    mem::MaybeUninit,
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::process::CommandExt,
    },
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const MAX_EXPANDED_SIZE: u64 = 1024 * 1024 * 1024;
const MAX_LISTING_SIZE: u64 = 8 * 1024 * 1024;
const DISK_RESERVE: u64 = 64 * 1024 * 1024;
const EXTRACTION_TIMEOUT: Duration = Duration::from_secs(300);

fn disk_space(staging: &Path) -> io::Result<(u128, u128)> {
    let directory = File::open(staging)?;
    let mut stat = MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::fstatvfs(directory.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let stat = unsafe { stat.assume_init() };
    let block_size = u128::from(stat.f_frsize);
    let available = u128::from(stat.f_bavail) * block_size;
    Ok((available.saturating_sub(u128::from(DISK_RESERVE)), block_size))
}

pub(super) fn check_download_space(staging: &Path, size: u64) -> Result<(), String> {
    let (available, _) = disk_space(staging)
        .map_err(|error| format!("Could not check space for the HoH release ({error})."))?;
    if u128::from(size) > available {
        return Err("There is not enough disk space to download the HoH release.".into());
    }
    Ok(())
}

fn disk_budget(staging: &Path) -> io::Result<(u64, u64)> {
    let (budget, block_size) = disk_space(staging)?;
    let limit = budget.min(u128::from(MAX_EXPANDED_SIZE)) as u64;
    let path_cost = (block_size.max(4096) * 4).min(u128::from(MAX_EXPANDED_SIZE)) as u64;
    Ok((limit, path_cost))
}

fn extractor(archive: &Path, staging: &Path, flags: &str) -> Command {
    let mut command = Command::new("/usr/bin/tar");
    command
        .process_group(0)
        .arg(flags)
        .arg(archive)
        .arg("-C")
        .arg(staging)
        .args(["--no-same-owner", "--no-same-permissions"])
        .env_remove("TAR_OPTIONS")
        .stdin(Stdio::null());
    #[cfg(target_os = "macos")]
    command.args(["--no-acls", "--no-xattrs", "--no-fflags", "--no-mac-metadata"]);
    #[cfg(target_os = "linux")]
    unsafe {
        command.pre_exec(|| {
            libc::umask(0o022);
            Ok(())
        });
    }
    command
}

#[cfg(target_os = "linux")]
pub(super) fn preserve_access_modes(installed: &Path, staged: &Path) -> io::Result<()> {
    use std::{
        fs,
        os::unix::fs::{MetadataExt, PermissionsExt, chown},
    };

    let previous = match installed.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let next = staged.symlink_metadata()?;
    if previous.is_dir() && next.is_dir() {
        for entry in fs::read_dir(staged)? {
            let entry = entry?;
            preserve_access_modes(&installed.join(entry.file_name()), &entry.path())?;
        }
    } else if !(previous.is_file() && next.is_file()) {
        return Ok(());
    }
    let mode = previous.mode() & 0o777;
    if next.gid() != previous.gid() && ((mode >> 3) & 0o7) != (mode & 0o7) {
        chown(staged, None, Some(previous.gid()))?;
    }
    fs::set_permissions(staged, fs::Permissions::from_mode(mode))
}

fn measure(
    child: &mut Child,
    budget: u64,
    path_cost: u64,
    stopped: impl Fn() -> bool,
) -> io::Result<u64> {
    let mut streams = [
        File::from(OwnedFd::from(child.stdout.take().expect("tar stdout is piped"))),
        File::from(OwnedFd::from(child.stderr.take().expect("tar stderr is piped"))),
    ];
    let mut descriptors = streams.each_ref().map(|stream| libc::pollfd {
        fd: stream.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    });
    let mut listing_remaining = MAX_LISTING_SIZE;
    let mut remaining = budget;
    let mut buffer = [0; 64 * 1024];
    while descriptors.iter().any(|descriptor| descriptor.fd >= 0) {
        if stopped() {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let count = descriptors.len() as libc::nfds_t;
        if unsafe { libc::poll(descriptors.as_mut_ptr(), count, 75) } < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        for (index, (descriptor, stream)) in descriptors.iter_mut().zip(&mut streams).enumerate() {
            if descriptor.revents == 0 {
                continue;
            }
            let count = stream.read(&mut buffer)?;
            if count == 0 {
                descriptor.fd = -1;
                continue;
            }
            let cost = if index == 0 {
                count as u64
            } else {
                listing_remaining = listing_remaining
                    .checked_sub(count as u64)
                    .ok_or(io::ErrorKind::FileTooLarge)?;
                let path_parts = buffer[..count]
                    .iter()
                    .filter(|&&byte| matches!(byte, b'/' | b'\n'))
                    .count() as u64;
                path_parts.saturating_mul(path_cost)
            };
            remaining = remaining
                .checked_sub(cost)
                .ok_or(io::ErrorKind::FileTooLarge)?;
        }
    }
    Ok(budget - remaining)
}

pub(super) fn extract(
    archive: &Path,
    staging: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<(), String> {
    let deadline = Instant::now() + EXTRACTION_TIMEOUT;
    let stopped = || cancelled() || Instant::now() >= deadline;
    let space_error = |error| format!("Could not check space for the HoH release ({error}).");
    let (limit, path_cost) = disk_budget(staging).map_err(space_error)?;
    let start_error = |error| format!("Could not start the package extractor ({error}).");
    let wait_error = |error| format!("Could not complete package extraction ({error}).");
    let unpack_error = "Could not unpack the HoH release. On Linux, make sure zstd is installed.";
    let mut preview = extractor(archive, staging, "-xvOf")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(start_error)?;
    let size_error = "The expanded HoH release exceeds the extraction limit or available disk space.";
    let size = match measure(&mut preview, limit, path_cost, stopped) {
        Ok(size) => size,
        Err(error) => {
            let _ = wait_for_group(&mut preview, || true);
            return Err(if error.kind() == io::ErrorKind::FileTooLarge {
                size_error.into()
            } else {
                wait_error(error)
            });
        }
    };
    if !wait_for_group(&mut preview, stopped).map_err(wait_error)?.success() {
        return Err(unpack_error.into());
    }
    if size > disk_budget(staging).map_err(space_error)?.0 {
        return Err(size_error.into());
    }
    let mut child = extractor(archive, staging, "-xf")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(start_error)?;
    if !wait_for_group(&mut child, stopped).map_err(wait_error)?.success() {
        return Err(unpack_error.into());
    }
    Ok(())
}
