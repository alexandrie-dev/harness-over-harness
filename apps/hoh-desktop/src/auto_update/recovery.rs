use super::{
    Package, candidate::initialize_engine_status, open_regular_file, read_install_metadata,
    swap_directories,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{self, Write},
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, PermissionsExt, symlink},
        },
    },
    path::{Path, PathBuf},
};

const PENDING: &str = "pending.json";
const ENGINE_STATUS: &str = "candidate-engine";
const ROLLBACK_SUFFIX: &str = "-rollback";
const RETIRED_PREFIX: &str = ".hoh-retired-";

#[derive(Deserialize, Serialize)]
struct PendingSwap {
    previous: u64,
    candidate: u64,
}

pub(super) struct Recovery {
    directory: PathBuf,
    pending: PendingSwap,
}

pub(super) fn sibling_path(installed: &Path, suffix: &str) -> io::Result<PathBuf> {
    let name = installed.file_name().ok_or(io::ErrorKind::InvalidInput)?;
    let key = Sha256::digest(name.as_bytes());
    let key = hoh_protocol::approval::hex(&key[..12]);
    let parent = installed.parent().ok_or(io::ErrorKind::InvalidInput)?;
    Ok(parent.join(format!(".hoh-{key}{suffix}")))
}

pub(super) fn is_backup(bundle: &Path) -> bool {
    bundle
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.starts_with(RETIRED_PREFIX)
                || name
                    .strip_prefix(".hoh-")
                    .and_then(|name| name.strip_suffix(ROLLBACK_SUFFIX))
                    .is_some_and(|key| {
                        key.len() == 24 && key.bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
        })
}

impl Recovery {
    #[cfg(target_os = "linux")]
    pub(super) fn check_exchange_permissions(installed: &Path, staged: &Path) -> io::Result<()> {
        let directory = parent(installed)?;
        for (path, mode) in [
            (directory, libc::W_OK | libc::X_OK),
            (installed, libc::W_OK),
            (staged, libc::W_OK),
        ] {
            let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
            if unsafe { libc::faccessat(libc::AT_FDCWD, path.as_ptr(), mode, libc::AT_EACCESS) } < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        let directory = directory.metadata()?;
        let user = unsafe { libc::geteuid() };
        if directory.mode() & libc::S_ISVTX != 0
            && user != 0
            && user != directory.uid()
            && user != installed.metadata()?.uid()
        {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok(())
    }

    pub(super) fn open(installed: &Path) -> io::Result<Option<Self>> {
        let directory = sibling_path(installed, ROLLBACK_SUFFIX)?;
        let metadata = match read_install_metadata(&directory.join(PENDING)) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let pending = serde_json::from_str(&metadata)?;
        Ok(Some(Self { directory, pending }))
    }

    pub(super) fn publish(
        installed: &Path,
        staging: &Path,
        package: Package,
        previous: &File,
    ) -> io::Result<(Self, File)> {
        let directory = sibling_path(installed, ROLLBACK_SUFFIX)?;
        if Self::open(installed)?.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "an earlier update is pending",
            ));
        }
        match directory.symlink_metadata() {
            Ok(_) => retire_directory(&directory)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let unpacked = staging.join("unpacked");
        let staged_bundle = staging.join(package.install_dir);
        fs::rename(package.staged_in(&unpacked)?, &staged_bundle)?;
        fs::remove_dir_all(unpacked)?;
        fs::remove_file(staging.join(package.asset_name()))?;
        let candidate = File::open(&staged_bundle)?;
        candidate.try_lock()?;
        sync_tree(&staged_bundle)?;
        let pending = PendingSwap {
            previous: previous.metadata()?.ino(),
            candidate: candidate.metadata()?.ino(),
        };
        let mut marker = File::create_new(staging.join(PENDING))?;
        marker.set_permissions(fs::Permissions::from_mode(0o644))?;
        marker.write_all(&serde_json::to_vec(&pending)?)?;
        marker.sync_all()?;
        fs::set_permissions(staging, fs::Permissions::from_mode(0o755))?;
        sync_directory(staging)?;
        fs::rename(staging, &directory)?;
        sync_directory(parent(installed)?)?;
        Ok((Self { directory, pending }, candidate))
    }

    pub(super) fn backup(&self, package: Package) -> PathBuf {
        self.directory.join(package.install_dir)
    }

    pub(super) fn prepare_engine_status(&self) -> io::Result<File> {
        let temporary = tempfile::NamedTempFile::new_in(&self.directory)?;
        let status = temporary.as_file();
        status.set_permissions(fs::Permissions::from_mode(0o644))?;
        initialize_engine_status(status)?;
        let status = temporary
            .persist_noclobber(self.directory.join(ENGINE_STATUS))
            .map_err(|error| error.error)?;
        sync_directory(&self.directory)?;
        Ok(status)
    }

    pub(super) fn engine_status(&self) -> io::Result<Option<File>> {
        match open_regular_file(&self.directory.join(ENGINE_STATUS)) {
            Ok(status) => Ok(Some(status)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn decision_path(&self) -> PathBuf {
        self.directory.join("decision")
    }

    pub(super) fn commit(&self) -> io::Result<()> {
        symlink("commit", self.decision_path())
    }

    pub(super) fn committed(&self) -> io::Result<bool> {
        match fs::read_link(self.decision_path()) {
            Ok(decision) if decision == Path::new("commit") => Ok(true),
            Ok(decision) if decision == Path::new("rollback") => Ok(false),
            Ok(_) => Err(io::ErrorKind::InvalidData.into()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub(super) fn is_swapped(&self, active: &File, backup: &File) -> io::Result<bool> {
        let inodes = (active.metadata()?.ino(), backup.metadata()?.ino());
        if inodes == (self.pending.candidate, self.pending.previous) {
            Ok(true)
        } else if inodes == (self.pending.previous, self.pending.candidate) {
            Ok(false)
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the pending update directories have changed",
            ))
        }
    }

    pub(super) fn exchange(&self, installed: &Path, package: Package) -> io::Result<()> {
        swap_directories(installed, &self.backup(package))
    }

    pub(super) fn sync_exchange(&self, installed: &Path) -> io::Result<()> {
        sync_directory(&self.directory)?;
        sync_directory(parent(installed)?)
    }

    pub(super) fn finish(&self) -> io::Result<()> {
        retire_directory(&self.directory)
    }
}

fn retire_directory(directory: &Path) -> io::Result<()> {
    let parent = parent(directory)?;
    let retired = tempfile::Builder::new()
        .prefix(RETIRED_PREFIX)
        .tempdir_in(parent)?;
    fs::rename(directory, retired.path())?;
    let retired = retired.keep();
    sync_directory(parent)?;
    if let Err(error) = fs::remove_dir_all(&retired) {
        eprintln!("HoH could not delete {}: {error}", retired.display());
    }
    Ok(())
}

fn parent(path: &Path) -> io::Result<&Path> {
    path.parent().ok_or_else(|| io::ErrorKind::InvalidInput.into())
}

fn sync_directory(path: &Path) -> io::Result<()> {
    let directory = File::open(path)?;
    loop {
        if unsafe { libc::fsync(directory.as_raw_fd()) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn sync_tree(path: &Path) -> io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            sync_tree(&entry.path())?;
        } else if kind.is_file() {
            File::open(entry.path())?.sync_all()?;
        }
    }
    sync_directory(path)
}
