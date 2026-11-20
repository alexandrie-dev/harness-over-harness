use getrandom::fill;
use hoh_error::BoxError;
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    fmt, io,
    path::{Path, PathBuf},
    sync::OnceLock,
};
use tokio::io::AsyncWriteExt;

pub const DEFAULT_ROOT_PREFIX: &str = "dsh-spill-";

static DEFAULT_ROOT: OnceLock<String> = OnceLock::new();

#[derive(Debug)]
struct NodeFsOperationError {
    source: io::Error,
    syscall: &'static str,
    path: String,
}

impl fmt::Display for NodeFsOperationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&hoh_error::node_fs_error_message(
            &self.source,
            self.syscall,
            Some(&self.path),
            None,
        ))
    }
}

impl Error for NodeFsOperationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

fn fs_operation_error(error: io::Error, syscall: &'static str, path: &Path) -> BoxError {
    Box::new(NodeFsOperationError {
        source: error,
        syscall,
        path: path.to_string_lossy().into_owned(),
    })
}

pub fn is_errno(error: &io::Error, code: &str) -> bool {
    hoh_error::node_error_code(error) == Some(code)
}

pub fn private_root() -> Result<String, BoxError> {
    if let Some(root) = DEFAULT_ROOT.get() {
        return Ok(root.clone());
    }
    let created = create_private_root()?;
    if DEFAULT_ROOT.set(created.clone()).is_err() {
        let _ = std::fs::remove_dir(&created);
    }
    Ok(DEFAULT_ROOT.get().cloned().unwrap_or(created))
}

fn create_private_root() -> Result<String, BoxError> {
    const ALPHANUMERIC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let temp_dir = std::env::temp_dir();
    let error_path = temp_dir.join(format!("{DEFAULT_ROOT_PREFIX}XXXXXX"));
    for _ in 0..128 {
        let mut random = [0_u8; 6];
        fill(&mut random).map_err(io::Error::other)?;
        let suffix: String = random
            .into_iter()
            .map(|byte| ALPHANUMERIC[usize::from(byte) % ALPHANUMERIC.len()] as char)
            .collect();
        let path = temp_dir.join(format!("{DEFAULT_ROOT_PREFIX}{suffix}"));
        match create_owner_directory(&path) {
            Ok(()) => return Ok(path.to_string_lossy().into_owned()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(fs_operation_error(error, "mkdtemp", &error_path)),
        }
    }
    Err(fs_operation_error(
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create a unique spill root",
        ),
        "mkdtemp",
        &error_path,
    ))
}

fn create_owner_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700).create(path)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir(path)
    }
}

pub fn encode_segment(raw: &str) -> String {
    if raw.is_empty() {
        return "~".into();
    }
    if raw == "." {
        return "~002E".into();
    }
    if raw == ".." {
        return "~002E~002E".into();
    }
    let mut encoded = String::new();
    for unit in raw.encode_utf16() {
        if unit <= 0x7f {
            let byte = unit as u8;
            if byte != b'~' && (byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
            {
                encoded.push(char::from(byte));
                continue;
            }
        }
        encoded.push_str(&format!("~{unit:04X}"));
    }
    encoded
}

pub fn session_dir(root: impl AsRef<Path>, session_id: &str) -> PathBuf {
    let digest = Sha256::digest(session_id.as_bytes());
    let hash = digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    root.as_ref().join(format!("session-{hash}"))
}

#[derive(Clone)]
pub struct SaveTextOptions {
    pub root: String,
    pub session_id: String,
    pub suggested_name: String,
    pub content: String,
}

impl fmt::Debug for SaveTextOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SaveTextOptions")
            .field("root", &self.root)
            .field("session_id", &self.session_id)
            .field("suggested_name", &self.suggested_name)
            .field("content_len", &self.content.len())
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedText {
    pub path: String,
    pub bytes: u64,
}

pub async fn save_text_file(options: SaveTextOptions) -> Result<SavedText, BoxError> {
    let directory = session_dir(&options.root, &options.session_id);
    let mut random = [0_u8; 6];
    fill(&mut random)?;
    let nonce = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let path = directory.join(format!(
        "{nonce}-{}",
        encode_segment(&options.suggested_name)
    ));
    loop {
        let mut builder = tokio::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o700);
        builder
            .create(&directory)
            .await
            .map_err(|error| fs_operation_error(error, "mkdir", &directory))?;
        let mut open = tokio::fs::OpenOptions::new();
        open.write(true).create_new(true);
        #[cfg(unix)]
        {
            open.mode(0o600);
        }
        match open.open(&path).await {
            Ok(mut file) => {
                file.write_all(options.content.as_bytes())
                    .await
                    .map_err(|error| fs_operation_error(error, "write", &path))?;
                file.flush()
                    .await
                    .map_err(|error| fs_operation_error(error, "write", &path))?;
                drop(file);
                return Ok(SavedText {
                    path: path.to_string_lossy().into_owned(),
                    bytes: options.content.len() as u64,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(fs_operation_error(error, "open", &path)),
        }
    }
}
