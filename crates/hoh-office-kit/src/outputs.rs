//! Private, bounded, owner-scoped files for converted and recalculated Office output.

use crate::{OfficeKitError, OfficeKitErrorCode};
use hoh_error::BoxError;
use indexmap::IndexMap;
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, SystemTime},
};
use tokio::{io::AsyncWriteExt, task::AbortHandle};

#[derive(Clone, Debug)]
pub struct OutputStoreOptions {
    pub root: PathBuf,
    pub retention: Duration,
    pub max_bytes: u64,
}

#[derive(Clone)]
struct Entry {
    id: u64,
    path: PathBuf,
    directory: PathBuf,
    owner: String,
    bytes: u64,
    timer: AbortHandle,
}

#[derive(Default)]
struct State {
    entries: IndexMap<PathBuf, Entry>,
    bytes: u64,
    next_id: u64,
    disposed: bool,
    swept: bool,
}

struct Inner {
    options: OutputStoreOptions,
    warn: Arc<dyn Fn(String) + Send + Sync>,
    cutoff: SystemTime,
    state: Mutex<State>,
    operations: tokio::sync::Mutex<()>,
}

#[derive(Clone)]
pub struct OutputStore(Arc<Inner>);

impl std::fmt::Debug for OutputStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OutputStore")
            .field("root", &self.0.options.root)
            .finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl OutputStore {
    pub fn new(options: OutputStoreOptions, warn: impl Fn(String) + Send + Sync + 'static) -> Self {
        let cutoff = SystemTime::now()
            .checked_sub(options.retention)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let inner = Arc::new(Inner {
            options,
            warn: Arc::new(warn),
            cutoff,
            state: Mutex::new(State::default()),
            operations: tokio::sync::Mutex::new(()),
        });
        // Like the TS constructor, sweep stale output of earlier processes right away;
        // without a runtime the sweep is deferred to the first operation.
        if tokio::runtime::Handle::try_current().is_ok() {
            let weak = Arc::downgrade(&inner);
            tokio::spawn(async move {
                if let Some(inner) = weak.upgrade() {
                    let _operation = inner.operations.lock().await;
                    inner.sweep_once().await;
                }
            });
        }
        Self(inner)
    }

    pub async fn save(
        &self,
        owner: &str,
        name: &str,
        data: &[u8],
    ) -> std::result::Result<PathBuf, BoxError> {
        let _operation = self.0.operations.lock().await;
        self.0.sweep_once().await;
        if lock(&self.0.state).disposed {
            return Err(Box::new(OfficeKitError::new(
                OfficeKitErrorCode::Unavailable,
                "The document converter is unavailable.",
            )));
        }
        let data_len = data.len() as u64;
        if data_len > self.0.options.max_bytes {
            return Err(Box::new(OfficeKitError::new(
                OfficeKitErrorCode::OutputTooLarge,
                format!(
                    "The output exceeds the {}-byte output store limit.",
                    self.0.options.max_bytes
                ),
            )));
        }
        // One pass over a snapshot: an undeletable entry must not be retried forever.
        let snapshot: Vec<Entry> = lock(&self.0.state).entries.values().cloned().collect();
        for candidate in snapshot {
            if lock(&self.0.state).bytes + data_len <= self.0.options.max_bytes {
                break;
            }
            self.0.remove_entry(candidate, false).await;
        }
        if lock(&self.0.state).bytes + data_len > self.0.options.max_bytes {
            return Err(Box::new(OfficeKitError::new(
                OfficeKitErrorCode::Unavailable,
                "Earlier outputs could not be deleted to make room for this output.",
            )));
        }
        let owner_dir = self.0.options.root.join(owner_directory(owner));
        let directory = owner_dir.join(random_uuid()?);
        let file_name = Path::new(name)
            .file_name()
            .unwrap_or_else(|| OsStr::new(""));
        let path = directory.join(file_name);
        if let Err(first) = write_output(&directory, &path, data).await {
            if first.kind() != std::io::ErrorKind::NotFound {
                return Err(Box::new(first));
            }
            write_output(&directory, &path, data).await?;
        }
        let id = {
            let mut state = lock(&self.0.state);
            let id = state.next_id;
            state.next_id = state.next_id.wrapping_add(1);
            id
        };
        let weak = Arc::downgrade(&self.0);
        let retention = self.0.options.retention;
        let timer_path = path.clone();
        let timer = tokio::spawn(async move {
            tokio::time::sleep(retention).await;
            if let Some(inner) = weak.upgrade() {
                inner.remove_path(timer_path, id).await;
            }
        })
        .abort_handle();
        let entry = Entry {
            id,
            path: path.clone(),
            directory,
            owner: owner.to_owned(),
            bytes: data_len,
            timer,
        };
        let mut state = lock(&self.0.state);
        state.bytes += data_len;
        state.entries.insert(path.clone(), entry);
        Ok(path)
    }

    pub async fn release(&self, owner: &str) {
        let _operation = self.0.operations.lock().await;
        let entries: Vec<_> = lock(&self.0.state)
            .entries
            .values()
            .filter(|entry| entry.owner == owner)
            .cloned()
            .collect();
        for entry in entries {
            self.0.remove_entry(entry, false).await;
        }
        let _ = tokio::fs::remove_dir(self.0.options.root.join(owner_directory(owner))).await;
    }

    pub async fn dispose(&self) {
        let _operation = self.0.operations.lock().await;
        {
            let mut state = lock(&self.0.state);
            state.disposed = true;
        }
        self.0.sweep_once().await;
        let entries: Vec<_> = lock(&self.0.state).entries.values().cloned().collect();
        let owners: Vec<_> = entries.iter().map(|entry| entry.owner.clone()).collect();
        for entry in entries {
            self.0.remove_entry(entry, false).await;
        }
        for owner in owners {
            let _ = tokio::fs::remove_dir(self.0.options.root.join(owner_directory(&owner))).await;
        }
    }
}

impl Inner {
    async fn remove_path(self: Arc<Self>, path: PathBuf, id: u64) {
        let _operation = self.operations.lock().await;
        let entry = lock(&self.state)
            .entries
            .get(&path)
            .filter(|entry| entry.id == id)
            .cloned();
        if let Some(entry) = entry {
            // The timer task is the caller: it must not abort itself.
            self.remove_entry(entry, true).await;
        }
    }

    async fn remove_entry(&self, entry: Entry, from_timer: bool) -> bool {
        if lock(&self.state)
            .entries
            .get(&entry.path)
            .is_none_or(|current| current.id != entry.id)
        {
            return true;
        }
        match tokio::fs::remove_dir_all(&entry.directory).await {
            Ok(()) => {
                if !from_timer {
                    entry.timer.abort();
                }
                let mut state = lock(&self.state);
                if state
                    .entries
                    .get(&entry.path)
                    .is_some_and(|current| current.id == entry.id)
                {
                    state.entries.shift_remove(&entry.path);
                    state.bytes -= entry.bytes;
                }
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !from_timer {
                    entry.timer.abort();
                }
                let mut state = lock(&self.state);
                if state
                    .entries
                    .get(&entry.path)
                    .is_some_and(|current| current.id == entry.id)
                {
                    state.entries.shift_remove(&entry.path);
                    state.bytes -= entry.bytes;
                }
                true
            }
            Err(error) => {
                (self.warn)(format!(
                    "office-kit: could not delete output {}: {error}",
                    entry.path.display()
                ));
                false
            }
        }
    }

    async fn sweep_once(&self) {
        {
            let mut state = lock(&self.state);
            if state.swept {
                return;
            }
            state.swept = true;
        }
        let cutoff = self.cutoff;
        for owner in generated_directories(&self.options.root, is_owner_directory).await {
            for directory in generated_directories(&owner, is_output_directory).await {
                let result = async {
                    let metadata = tokio::fs::symlink_metadata(&directory).await?;
                    if metadata.modified()? < cutoff {
                        tokio::fs::remove_dir_all(&directory).await?;
                    }
                    std::io::Result::Ok(())
                }
                .await;
                if let Err(error) = result {
                    (self.warn)(format!(
                        "office-kit: could not sweep output {}: {error}",
                        directory.display()
                    ));
                }
            }
            let _ = tokio::fs::remove_dir(owner).await;
        }
    }
}

async fn write_output(directory: &Path, path: &Path, data: &[u8]) -> std::io::Result<()> {
    let result = async {
        // Owner-only for every created level (root, owner and output directory).
        let mut builder = tokio::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o700);
        builder.create(directory).await?;
        #[cfg(unix)]
        tokio::fs::set_permissions(
            directory,
            <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
        )
        .await?;
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            options.mode(0o600);
        }
        let mut file = options.open(path).await?;
        file.write_all(data).await?;
        file.flush().await?;
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_dir_all(directory).await;
    }
    result
}

async fn generated_directories(directory: &Path, accepts: fn(&str) -> bool) -> Vec<PathBuf> {
    let Ok(mut entries) = tokio::fs::read_dir(directory).await else {
        return Vec::new();
    };
    let mut found = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if accepts(name) && entry.file_type().await.is_ok_and(|kind| kind.is_dir()) {
            found.push(entry.path());
        }
    }
    found.sort_by(|left, right| left.as_os_str().cmp(right.as_os_str()));
    found
}

fn owner_directory(owner: &str) -> String {
    let digest = Sha256::digest(owner.as_bytes());
    format!("session-{}", hex(&digest[..12]))
}

fn random_uuid() -> std::result::Result<String, BoxError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| std::io::Error::other(error.to_string()))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let value = hex(&bytes);
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &value[..8],
        &value[8..12],
        &value[12..16],
        &value[16..20],
        &value[20..]
    ))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

fn is_owner_directory(name: &str) -> bool {
    name.len() == 32
        && name.starts_with("session-")
        && name[8..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn is_output_directory(name: &str) -> bool {
    name.len() == 36
        && name.as_bytes().get(8) == Some(&b'-')
        && name.as_bytes().get(13) == Some(&b'-')
        && name.as_bytes().get(14) == Some(&b'4')
        && name.as_bytes().get(18) == Some(&b'-')
        && name.as_bytes().get(23) == Some(&b'-')
        && matches!(name.as_bytes().get(19), Some(b'8' | b'9' | b'a' | b'b'))
        && name.bytes().enumerate().all(|(index, byte)| {
            matches!(index, 8 | 13 | 18 | 23)
                || byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
        })
}
