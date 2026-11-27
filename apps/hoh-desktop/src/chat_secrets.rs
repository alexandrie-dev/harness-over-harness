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
const TURN_LIFETIME: Duration = Duration::from_secs(5 * 60);
const TURN_PREFIX: &str = "turn-";
const MAX_BYTES: usize = 64 * 1024;
const MAX_FILES: usize = 128;

struct SecretFile {
    path: PathBuf,
    expires: Instant,
    session_id: Option<String>,
}

pub struct ChatSecrets {
    directory: PathBuf,
    files: Mutex<Option<Vec<SecretFile>>>,
}

impl ChatSecrets {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            files: Mutex::new(Some(Vec::new())),
        }
    }

    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn store(&self, secret: &SecretString) -> Result<String, String> {
        self.store_file(secret, None)
    }

    pub fn store_for_turn(
        &self,
        session_id: &str,
        secret: &SecretString,
    ) -> Result<String, String> {
        self.store_file(secret, Some(session_id))
    }

    fn store_file(
        &self,
        secret: &SecretString,
        session_id: Option<&str>,
    ) -> Result<String, String> {
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
        let (prefix, lifetime) = if session_id.is_some() {
            (TURN_PREFIX, TURN_LIFETIME)
        } else {
            ("", LIFETIME)
        };
        let path = self
            .write(value, prefix)
            .map_err(|_| "Could not save the temporary secret.")?;
        let Some(reference) = path.to_str().map(str::to_owned) else {
            let _ = fs::remove_file(&path);
            return Err("Could not prepare the temporary secret path.".into());
        };
        files.push(SecretFile {
            path,
            expires: now + lifetime,
            session_id: session_id.map(str::to_owned),
        });
        Ok(reference)
    }

    fn write(&self, value: &str, prefix: &str) -> io::Result<PathBuf> {
        prepare_directory(&self.directory)?;
        let mut random = [0; 32];
        File::open("/dev/urandom")?.read_exact(&mut random)?;
        let path = self
            .directory
            .join(format!("{prefix}{}", hoh_protocol::approval::hex(&random)));
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
                    let name = entry.file_name();
                    let Some(lifetime) = name.to_str().and_then(file_lifetime) else {
                        continue;
                    };
                    if entry.metadata().is_ok_and(|meta| {
                        meta.is_file()
                            && meta
                                .modified()
                                .ok()
                                .and_then(|time| time.elapsed().ok())
                                .is_some_and(|age| age >= lifetime)
                    }) {
                        let _ = fs::remove_file(entry.path());
                    }
                }
            }
        }
    }

    pub fn discard(&self, path: &Path) {
        self.remove_matching(|file| file.path == path);
    }

    pub fn close_session(&self, session_id: &str) {
        self.remove_matching(|file| file.session_id.as_deref() == Some(session_id));
    }

    pub fn close_turns(&self) {
        self.remove_matching(|file| file.session_id.is_some());
    }

    fn remove_matching(&self, predicate: impl Fn(&SecretFile) -> bool) {
        let mut files = self.files.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(files) = files.as_mut() {
            files.retain_mut(|file| {
                if !predicate(file) {
                    return true;
                }
                file.expires = Instant::now();
                !remove_file(&file.path)
            });
        }
    }

    pub fn close(&self) {
        let mut files = self.files.lock().unwrap_or_else(|p| p.into_inner());
        for file in files.take().into_iter().flatten() {
            let _ = fs::remove_file(file.path);
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

fn file_lifetime(name: &str) -> Option<Duration> {
    let (nonce, lifetime) = name
        .strip_prefix(TURN_PREFIX)
        .map_or((name, LIFETIME), |nonce| (nonce, TURN_LIFETIME));
    (nonce.len() == 64 && nonce.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(lifetime)
}

fn remove_file(path: &Path) -> bool {
    !fs::remove_file(path).is_err_and(|error| error.kind() != io::ErrorKind::NotFound)
}

fn expire(files: &mut Vec<SecretFile>, now: Instant) {
    files.retain(|file| file.expires > now || !remove_file(&file.path));
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

        store.files.lock().unwrap().as_mut().unwrap()[0].expires = Instant::now();
        store.clean_expired();
        assert!(!Path::new(&first).exists());
        assert!(Path::new(&second).is_file());
        store.close();
        assert!(!Path::new(&second).exists());
        assert!(store.store(&value.into()).is_err());
        fs::remove_dir(directory).unwrap();
    }
}
