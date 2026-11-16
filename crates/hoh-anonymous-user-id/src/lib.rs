//! Anonymous per-home identity, synchronously persisted and memoized by its owner.
use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};
hoh_brand::branded_string!(pub AnonymousUserId);
pub const ANONYMOUS_USER_ID_FILE_NAME: &str = ".anonymous-user-id";

#[derive(Default)]
pub struct AnonymousUserIdOptions<'a> {
    pub env: Option<&'a HashMap<String, String>>,
    pub random_uuid: Option<&'a dyn Fn() -> String>,
}
#[derive(Debug, thiserror::Error)]
pub enum AnonymousUserIdError {
    #[error(transparent)]
    Home(#[from] io::Error),
}
/// Keep one instance in the process context and share it with all consumers.
/// A mutex preserves the source's indivisible synchronous read/create/memoize sequence.
#[derive(Default)]
pub struct AnonymousUserIdStore {
    memo: Mutex<HashMap<PathBuf, AnonymousUserId>>,
}

fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
fn read_persisted_id(file: &Path) -> Option<AnonymousUserId> {
    let bytes = fs::read(file).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let value = text.trim_matches(|c| matches!(c, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'));
    valid_uuid(value).then(|| value.into())
}

impl AnonymousUserIdStore {
    /// Performs blocking file I/O under the memo lock; async callers must use `spawn_blocking`.
    pub fn get_or_create_anonymous_user_id(
        &self,
        options: AnonymousUserIdOptions<'_>,
    ) -> Result<AnonymousUserId, AnonymousUserIdError> {
        let file =
            hoh_home_paths::resolve_dsh_home(None, options.env)?.join(ANONYMOUS_USER_ID_FILE_NAME);
        // Poisoning does not invalidate entries already committed to this map.
        let mut memo = self.memo.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(id) = memo.get(&file) {
            return Ok(id.clone());
        }
        let id = if let Some(id) = read_persisted_id(&file) {
            id
        } else {
            let created: AnonymousUserId = match options.random_uuid {
                Some(generate) => generate().into(),
                None => String::from(hoh_util_crypto::random_uuid()).into(),
            };
            let persisted = (|| -> io::Result<()> {
                if let Some(parent) = file.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&file)?
                    .write_all(format!("{created}\n").as_bytes())
            })();
            if persisted.is_ok() {
                created
            } else if let Some(winner) = read_persisted_id(&file) {
                winner
            } else {
                let _ = fs::write(&file, format!("{created}\n"));
                created
            }
        };
        memo.insert(file, id.clone());
        Ok(id)
    }
}
pub fn get_or_create_anonymous_user_id(
    store: &AnonymousUserIdStore,
    options: AnonymousUserIdOptions<'_>,
) -> Result<AnonymousUserId, AnonymousUserIdError> {
    store.get_or_create_anonymous_user_id(options)
}
