//! Temporary files for secrets handed to an agent's commands. The chat receives only the path.
use hoh_protocol::SecretString;
use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

const LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_BYTES: usize = 64 * 1024;
const MAX_FILES: usize = 128;

pub struct ChatSecrets {
    directory: PathBuf,
    files: Mutex<Option<Vec<(PathBuf, Instant)>>>,
}

impl ChatSecrets {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            files: Mutex::new(Some(Vec::new())),
        }
    }

    pub fn store(&self, secret: &SecretString) -> Result<String, String> {
        let value = secret.expose();
        if value.len() > MAX_BYTES || value.trim().is_empty() {
            return Err("The secret must be non-empty and at most 64 KiB.".into());
        }
        let mut files = self.files.lock().unwrap_or_else(|p| p.into_inner());
        let files = files.as_mut().ok_or("The window is closing.")?;
        let now = Instant::now();
        expire(files, now);
        if files.len() >= MAX_FILES {
            return Err("Too many temporary secrets are still active.".into());
        }
        let path = self
            .write(value)
            .map_err(|_| "Could not save the temporary secret.")?;
        let Some(reference) = path.to_str().map(str::to_owned) else {
            let _ = fs::remove_file(&path);
            return Err("Could not prepare the temporary secret path.".into());
        };
        files.push((path, now + LIFETIME));
        Ok(reference)
    }

    fn write(&self, value: &str) -> io::Result<PathBuf> {
        prepare_directory(&self.directory)?;
        let mut random = [0; 32];
        File::open("/dev/urandom")?.read_exact(&mut random)?;
        let path = self.directory.join(hoh_protocol::approval::hex(&random));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        if let Err(error) = file.write_all(value.as_bytes()) {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(error);
        }
        Ok(path)
    }

    pub fn clean_expired(&self) {
        let mut files = self.files.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(files) = files.as_mut() {
            expire(files, Instant::now());
            // Also sweep files left by a process that exited without shutting down.
            if prepare_directory(&self.directory).is_ok()
                && let Ok(entries) = fs::read_dir(&self.directory)
            {
                for entry in entries.flatten() {
                    let is_secret = entry.file_name().to_str().is_some_and(|name| {
                        name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit())
                    });
                    if is_secret
                        && entry.metadata().is_ok_and(|meta| {
                            meta.is_file()
                                && meta
                                    .modified()
                                    .ok()
                                    .and_then(|time| time.elapsed().ok())
                                    .is_some_and(|age| age >= LIFETIME)
                        })
                    {
                        let _ = fs::remove_file(entry.path());
                    }
                }
            }
        }
    }

    pub fn close(&self) {
        let mut files = self.files.lock().unwrap_or_else(|p| p.into_inner());
        for (path, _) in files.take().into_iter().flatten() {
            let _ = fs::remove_file(path);
        }
    }
}

impl Drop for ChatSecrets {
    fn drop(&mut self) {
        self.close();
    }
}

fn prepare_directory(path: &Path) -> io::Result<()> {
    match DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let meta = fs::symlink_metadata(path)?;
            if meta.is_dir() && meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o077 == 0
            {
                Ok(())
            } else {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            }
        }
        Err(error) => Err(error),
    }
}

fn expire(files: &mut Vec<(PathBuf, Instant)>, now: Instant) {
    files.retain(|(path, deadline)| {
        *deadline > now || fs::remove_file(path).is_err_and(|e| e.kind() != io::ErrorKind::NotFound)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn stores_exact_bytes_privately_and_cleans_up() {
        let mut random = [0; 32];
        File::open("/dev/urandom")
            .unwrap()
            .read_exact(&mut random)
            .unwrap();
        let directory =
            std::env::temp_dir().join(format!("hoh-test-{}", hoh_protocol::approval::hex(&random)));
        let store = ChatSecrets::new(directory.clone());
        assert!(store.store(&"  ".into()).is_err());
        assert!(store.store(&"x".repeat(MAX_BYTES + 1).into()).is_err());

        let value = "  test-only-token\nsecond line\n";
        let first = store.store(&value.into()).unwrap();
        let second = store.store(&value.into()).unwrap();
        assert_ne!(first, second);
        assert!(!first.contains("test-only-token"));
        assert_eq!(fs::read_to_string(&first).unwrap(), value);
        assert_eq!(
            fs::metadata(&first).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );

        store.files.lock().unwrap().as_mut().unwrap()[0].1 = Instant::now();
        store.clean_expired();
        assert!(!Path::new(&first).exists());
        assert!(Path::new(&second).is_file());
        store.close();
        assert!(!Path::new(&second).exists());
        assert!(store.store(&value.into()).is_err());
        fs::remove_dir(directory).unwrap();
    }
}
