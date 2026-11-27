use super::{Package, swap_directories};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{self, Write},
    os::{
        fd::AsRawFd,
        unix::{ffi::OsStrExt, fs::{MetadataExt, PermissionsExt}},
    },
    path::{Path, PathBuf},
};

const PENDING: &str = "pending.json";
const ROLLBACK_SUFFIX: &str = "-rollback";

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
        .and_then(|name| name.strip_prefix(".hoh-")?.strip_suffix(ROLLBACK_SUFFIX))
        .is_some_and(|key| key.len() == 24 && key.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

impl Recovery {
    pub(super) fn open(installed: &Path) -> io::Result<Option<Self>> {
        let directory = sibling_path(installed, ROLLBACK_SUFFIX)?;
        let bytes = match fs::read(directory.join(PENDING)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let pending = serde_json::from_slice(&bytes)?;
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
        match fs::remove_dir_all(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let unpacked = staging.join("unpacked");
        let staged_bundle = staging.join(package.install_dir);
        fs::rename(package.staged_in(&unpacked), &staged_bundle)?;
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

    pub(super) fn ready_path(&self) -> PathBuf {
        self.directory.join("ready")
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
        fs::remove_file(self.directory.join(PENDING))?;
        sync_directory(&self.directory)?;
        let _ = fs::remove_dir_all(&self.directory);
        Ok(())
    }
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
