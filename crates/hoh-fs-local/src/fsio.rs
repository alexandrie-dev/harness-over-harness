//! Local filesystem mechanics; Linux publication and strict UTF-8 decoding.
use hoh_error::{BoxError, HarnessError};
use hoh_fs::{
    ErrnoError, FsByteRange, FsError, FsErrorCode, FsFuture, FsPathType, FsTarget, FsTextStream,
    FsType, FsVersion, fs_error_from_errno,
};
use hoh_timeout::AbortSignal;
use hoh_util_values::js_trim;
use hoh_util_workspace_path::node_path::{self, basename, dirname, is_absolute, join};
use std::{
    io,
    os::{
        fd::{IntoRawFd, OwnedFd},
        unix::fs::{MetadataExt, PermissionsExt},
    },
    sync::Arc,
};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};

const BINARY_SAMPLE_BYTES: usize = 8192;
const READ_CHUNK: usize = 64 * 1024;
pub type LocalTarget = FsTarget;
#[derive(Clone, Debug)]
pub struct PathInfo {
    pub version: FsVersion,
    pub mode: u32,
    pub r#type: FsType,
    pub size: u64,
}
#[derive(Clone, Debug)]
pub struct PathLinkInfo {
    pub version: FsVersion,
    pub mode: u32,
    pub r#type: FsPathType,
    pub size: u64,
}
#[derive(Clone, Debug)]
pub struct LocalDirEntry {
    pub name: String,
    pub r#type: FsType,
    pub target: LocalTarget,
    pub version: Option<FsVersion>,
    pub size: Option<u64>,
}
#[derive(Clone, Debug)]
pub struct StagedPaths {
    pub staging_dir: String,
    pub temp_path: String,
}
pub type PathHook = Arc<dyn Fn(String) -> FsFuture<'static, ()> + Send + Sync>;
pub type TargetHook = Arc<dyn Fn(LocalTarget) -> FsFuture<'static, ()> + Send + Sync>;
pub type OperationHook = Arc<dyn Fn(&str, &str) -> Result<(), BoxError> + Send + Sync>;
pub type MetadataHook = Arc<dyn Fn(String, String) -> FsFuture<'static, ()> + Send + Sync>;
pub type SizeHook = Arc<dyn Fn(usize) + Send + Sync>;
pub type NameHook = Arc<dyn Fn(&str) -> Result<String, BoxError> + Send + Sync>;
pub type LinkHook = Arc<dyn Fn(String, String) -> FsFuture<'static, ()> + Send + Sync>;
pub type PublicationHook =
    Arc<dyn Fn(String) -> FsFuture<'static, Option<PathLinkInfo>> + Send + Sync>;
/// Replaces the result of a temp-file `close(2)` (the source's per-handle `close` spy).
pub type CloseHook = Arc<dyn Fn() -> Result<(), BoxError> + Send + Sync>;
pub type StagedHook = Arc<dyn Fn(StagedPaths) -> FsFuture<'static, ()> + Send + Sync>;
pub type DiffMetadataHook = Arc<dyn Fn(String) -> FsFuture<'static, PathInfo> + Send + Sync>;
pub type WatchLinkHook = Arc<dyn Fn(String) + Send + Sync>;
pub type WatchLinkGateHook = Arc<dyn Fn(String) -> bool + Send + Sync>;
pub type WatchAddHook = Arc<dyn Fn(String, bool) + Send + Sync>;
#[derive(Clone, Default)]
pub struct FsIoInternals {
    pub home_directory: Option<String>,
    pub after_metadata: Option<MetadataHook>,
    pub inspect_diff_read: Option<SizeHook>,
    pub inspect_diff_allocate: Option<SizeHook>,
    pub temp_dir_name: Option<NameHook>,
    pub temp_name: Option<NameHook>,
    pub link_file: Option<LinkHook>,
    pub inspect_publication_target: Option<PublicationHook>,
    pub inspect_before_mutation: Option<TargetHook>,
    pub remove_file: Option<PathHook>,
    pub remove_file_override: Option<LinkHook>,
    pub remove_staging_dir: Option<PathHook>,
    pub inspect_temp: Option<StagedHook>,
    /// Called after each temp-file close attempt (publication and cleanup); an `Err` replaces the result.
    pub close_file: Option<CloseHook>,
    pub inspect_read_bytes_after_stat: Option<TargetHook>,
    pub inspect_watch_metadata: Option<TargetHook>,
    /// Observes a successful native watch of a followed symlink.
    pub inspect_watch_link: Option<WatchLinkHook>,
    /// Test gate called with the native mutex held, immediately before watch(link).
    pub before_watch_link: Option<WatchLinkGateHook>,
    /// Observes an Add after dispatch (`false`) and after its callback (`true`).
    pub inspect_watch_add: Option<WatchAddHook>,
    // Rust counterparts of source specs' per-call Node fs/descriptor spies.
    pub before_operation: Option<OperationHook>,
    pub after_diff_open: Option<PathHook>,
    pub after_diff_stat: Option<PathHook>,
    pub after_diff_read: Option<PathHook>,
    pub diff_metadata: Option<DiffMetadataHook>,
}
pub(crate) fn abort(signal: Option<&AbortSignal>, verb: &str) -> Result<(), BoxError> {
    if signal.is_some_and(AbortSignal::aborted) {
        Err(FsError::new(format!("{verb} aborted"), FsErrorCode::Aborted).into())
    } else {
        Ok(())
    }
}
pub(crate) fn errno(
    error: io::Error,
    syscall: &str,
    path: Option<&str>,
    dest: Option<&str>,
) -> BoxError {
    let (first, second) = match syscall {
        "rename" => ("oldPath", "newPath"),
        "link" => ("existingPath", "newPath"),
        _ => ("path", "newPath"),
    };
    for (name, value) in [(first, path), (second, dest)] {
        if let Some(value) = value.filter(|v| v.contains('\0')) {
            return Box::new(null_bytes_error(name, value));
        }
    }
    Box::new(ErrnoError::from_io(error, syscall, path, dest))
}
/// Node's `ERR_INVALID_ARG_VALUE` for a path with a NUL byte (`fs/utils.js` `nullCheck`).
fn null_bytes_error(name: &str, value: &str) -> ErrnoError {
    let mut inspected = inspect_string(value);
    if inspected.encode_utf16().count() > 128 {
        let head: Vec<u16> = inspected.encode_utf16().take(128).collect();
        inspected = format!("{}...", String::from_utf16_lossy(&head));
    }
    ErrnoError::new(
        Some("ERR_INVALID_ARG_VALUE".into()),
        format!(
            "The argument '{name}' must be a string, Uint8Array, or URL without null bytes. Received {inspected}"
        ),
    )
}
/// `util.inspect(string)`: quote choice and escapes of `lib/internal/util/inspect.js` `strEscape`.
fn inspect_string(value: &str) -> String {
    let quote = if !value.contains('\'') {
        '\''
    } else if !value.contains('"') {
        '"'
    } else if !value.contains('`') && !value.contains("${") {
        '`'
    } else {
        '\''
    };
    let mut out = String::from(quote);
    for c in value.chars() {
        match c {
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            '\\' => out.push_str("\\\\"),
            c if c == quote && quote == '\'' => out.push_str("\\'"),
            c if (c as u32) < 32 || (127..160).contains(&(c as u32)) => {
                out.push_str(&format!("\\x{:02X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}
/// Closes a temp file through `close(2)` so its error is observed before publication.
async fn close_native(file: fs::File) -> Result<(), BoxError> {
    let fd = OwnedFd::from(file.into_std().await).into_raw_fd();
    match tokio::task::spawn_blocking(move || nix::unistd::close(fd)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(errno(io::Error::from(e), "close", None, None)),
        Err(e) => Err(io::Error::other(e.to_string()).into()),
    }
}
/// One `handle.close()` attempt. With no file left (the descriptor was released by a failed
/// attempt) only the observed result of the hook remains, as a second Node `close()` would repeat it.
async fn close_attempt(file: Option<fs::File>, internals: &FsIoInternals) -> Result<(), BoxError> {
    if let Some(file) = file {
        close_native(file).await?;
    }
    internals.close_file.as_ref().map_or(Ok(()), |h| h())
}
fn inner_error<'a>(
    error: &'a (dyn std::error::Error + 'static),
) -> &'a (dyn std::error::Error + 'static) {
    hoh_error::SharedError::of(error)
        .map(|e| e.get_ref() as &(dyn std::error::Error + 'static))
        .unwrap_or(error)
}
pub(crate) fn code<'a>(error: &'a (dyn std::error::Error + 'static)) -> Option<&'a str> {
    inner_error(error)
        .downcast_ref::<ErrnoError>()
        .and_then(|e| e.code.as_deref())
        .or_else(|| HarnessError::of(error).map(HarnessError::code))
}
fn missing(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}
fn hook(internals: &FsIoInternals, operation: &str, path: &str) -> Result<(), BoxError> {
    if let Some(h) = &internals.before_operation {
        h(operation, path)?;
    }
    Ok(())
}
fn current_dir() -> Result<String, BoxError> {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| errno(e, "uv_cwd", None, None))
}
#[allow(deprecated)] // std's Unix HOME/getpwuid behavior is Node os.homedir's contract.
pub fn expand_home_prefix(path: &str) -> Result<String, BoxError> {
    if path == "~" || path.starts_with("~/") {
        let home = std::env::home_dir().ok_or_else(|| {
            FsError::new("cannot determine the home directory", FsErrorCode::IoError)
        })?;
        Ok(format!("{}{}", home.to_string_lossy(), &path[1..]))
    } else {
        Ok(path.into())
    }
}
pub fn expand_home_prefix_with_override(
    path: &str,
    home: Option<&str>,
) -> Result<String, BoxError> {
    if (path == "~" || path.starts_with("~/"))
        && let Some(home) = home
    {
        Ok(format!("{home}{}", &path[1..]))
    } else {
        expand_home_prefix(path)
    }
}
pub fn local_display_path(cwd: &str, path: &str) -> Result<String, BoxError> {
    let base = if is_absolute(cwd) {
        cwd.to_owned()
    } else {
        format!("{}/{cwd}", current_dir()?)
    };
    let raw = if is_absolute(path) {
        path.to_owned()
    } else {
        format!("{base}/{path}")
    };
    if raw.split(['/', '\\']).any(|part| part == "..") {
        Ok(raw)
    } else {
        // Node's `resolve` consults `process.cwd()` only while the result is still relative.
        let base = if is_absolute(cwd) || is_absolute(path) {
            "/".to_owned()
        } else {
            current_dir()?
        };
        Ok(node_path::resolve(&base, &[cwd, path]))
    }
}
pub async fn resolve_local_target(cwd: &str, path: &str) -> Result<LocalTarget, BoxError> {
    resolve_local_target_with_internals(cwd, path, &FsIoInternals::default()).await
}
pub async fn resolve_local_target_with_internals(
    cwd: &str,
    path: &str,
    internals: &FsIoInternals,
) -> Result<LocalTarget, BoxError> {
    if js_trim(path).is_empty() {
        return Err(FsError::new(
            "file_path must be a non-empty string",
            FsErrorCode::NotFound,
        )
        .into());
    }
    let display_path = local_display_path(cwd, path)?;
    hook(internals, "realpath", &display_path)
        .map_err(|e| fs_error_from_errno("resolve", &display_path, e))?;
    match fs::canonicalize(&display_path).await {
        Ok(p) => {
            return Ok(LocalTarget {
                target_key: p.to_string_lossy().into_owned().into(),
                display_path,
            });
        }
        Err(e) if e.kind() == io::ErrorKind::NotADirectory => {
            return Err(FsError::new(
                format!(
                    "cannot resolve \"{display_path}\": a parent path segment is not a directory"
                ),
                FsErrorCode::NotFound,
            )
            .into());
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(fs_error_from_errno(
                "resolve",
                &display_path,
                errno(e, "realpath", Some(&display_path), None),
            ));
        }
    }
    let mut suffix = vec![basename(&display_path)];
    let mut ancestor = dirname(&display_path);
    loop {
        match fs::canonicalize(&ancestor).await {
            Ok(real) => {
                if suffix.iter().any(|s| s == "..") {
                    return Err(FsError::new(format!("cannot resolve \"{display_path}\": parent traversal crosses a missing directory"),FsErrorCode::NotFound).into());
                }
                let real = real.to_string_lossy();
                let mut pieces = vec![real.as_ref()];
                pieces.extend(suffix.iter().map(String::as_str));
                return Ok(LocalTarget {
                    target_key: join(&pieces).into(),
                    display_path,
                });
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let parent = dirname(&ancestor);
                if parent == ancestor {
                    return Ok(LocalTarget {
                        target_key: display_path.clone().into(),
                        display_path,
                    });
                }
                suffix.insert(0, basename(&ancestor));
                ancestor = parent;
            }
            Err(e) => {
                return Err(fs_error_from_errno(
                    "resolve",
                    &display_path,
                    errno(e, "realpath", Some(&ancestor), None),
                ));
            }
        }
    }
}
fn version_of(info: &std::fs::Metadata) -> FsVersion {
    let mtime = i128::from(info.mtime()) * 1_000_000_000 + i128::from(info.mtime_nsec());
    let ctime = i128::from(info.ctime()) * 1_000_000_000 + i128::from(info.ctime_nsec());
    format!(
        "{}:{}:{}:{mtime}:{ctime}",
        info.dev(),
        info.ino(),
        info.len()
    )
    .into()
}
fn path_type(m: &std::fs::Metadata) -> FsType {
    if m.is_file() {
        FsType::File
    } else if m.is_dir() {
        FsType::Directory
    } else {
        FsType::Other
    }
}
fn path_info(m: &std::fs::Metadata) -> PathInfo {
    PathInfo {
        version: version_of(m),
        mode: m.mode() & 0o777,
        r#type: path_type(m),
        size: m.len(),
    }
}
pub async fn probe(path: &str) -> Result<Option<PathInfo>, BoxError> {
    probe_with_internals(path, &FsIoInternals::default()).await
}
pub async fn probe_with_internals(
    path: &str,
    internals: &FsIoInternals,
) -> Result<Option<PathInfo>, BoxError> {
    hook(internals, "stat", path).map_err(|e| fs_error_from_errno("stat", path, e))?;
    match fs::metadata(path).await {
        Ok(m) => Ok(Some(path_info(&m))),
        Err(e) if missing(&e) => Ok(None),
        Err(e) => Err(fs_error_from_errno(
            "stat",
            path,
            errno(e, "stat", Some(path), None),
        )),
    }
}
pub async fn probe_no_follow(path: &str) -> Result<Option<PathLinkInfo>, BoxError> {
    match fs::symlink_metadata(path).await {
        Ok(m) => Ok(Some(PathLinkInfo {
            version: version_of(&m),
            mode: m.mode() & 0o777,
            r#type: if m.file_type().is_symlink() {
                FsPathType::Symlink
            } else {
                path_type(&m).into()
            },
            size: m.len(),
        })),
        Err(e) if missing(&e) => Ok(None),
        Err(e) => Err(fs_error_from_errno(
            "stat",
            path,
            errno(e, "lstat", Some(path), None),
        )),
    }
}
pub async fn list_directory(
    target: &LocalTarget,
    signal: Option<&AbortSignal>,
) -> Result<Vec<LocalDirEntry>, BoxError> {
    list_directory_with_internals(target, signal, &FsIoInternals::default()).await
}
pub async fn list_directory_with_internals(
    target: &LocalTarget,
    signal: Option<&AbortSignal>,
    internals: &FsIoInternals,
) -> Result<Vec<LocalDirEntry>, BoxError> {
    abort(signal, "list")?;
    let info = probe_with_internals(target.target_key.as_str(), internals)
        .await
        .map_err(|e| fs_error_from_errno("list", &target.display_path, e))?;
    let info = info.ok_or_else(|| {
        FsError::new(
            format!("cannot list \"{}\": not found", target.display_path),
            FsErrorCode::NotFound,
        )
    })?;
    if info.r#type != FsType::Directory {
        return Err(FsError::new(
            format!("cannot list \"{}\": not a directory", target.display_path),
            FsErrorCode::NotDirectory,
        )
        .into());
    }
    hook(internals, "scandir", target.target_key.as_str())
        .map_err(|e| fs_error_from_errno("list", &target.display_path, e))?;
    let mut reader = fs::read_dir(target.target_key.as_str())
        .await
        .map_err(|e| {
            fs_error_from_errno(
                "list",
                &target.display_path,
                errno(e, "scandir", Some(target.target_key.as_str()), None),
            )
        })?;
    let mut names = Vec::new();
    while let Some(entry) = reader.next_entry().await.map_err(|e| {
        fs_error_from_errno(
            "list",
            &target.display_path,
            errno(e, "scandir", Some(target.target_key.as_str()), None),
        )
    })? {
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    // Node readdir's byte order is the input to the stable localeCompare sort.
    names.sort();
    crate::shim::sort_locale(&mut names);
    abort(signal, "list")?;
    let mut result = Vec::new();
    for name in names {
        abort(signal, "list")?;
        let identity =
            resolve_local_target_with_internals(target.target_key.as_str(), &name, internals)
                .await
                .map_err(|e| {
                    fs_error_from_errno(
                        "list",
                        &local_display_path(&target.display_path, &name)
                            .unwrap_or_else(|_| name.clone()),
                        e,
                    )
                })?;
        let child = probe_with_internals(identity.target_key.as_str(), internals)
            .await
            .map_err(|e| {
                fs_error_from_errno(
                    "list",
                    &local_display_path(&target.display_path, &name)
                        .unwrap_or_else(|_| name.clone()),
                    e,
                )
            })?;
        result.push(LocalDirEntry {
            name: name.clone(),
            r#type: child.as_ref().map_or(FsType::Other, |c| c.r#type),
            target: LocalTarget {
                target_key: identity.target_key,
                display_path: local_display_path(&target.display_path, &name)?,
            },
            version: child.as_ref().map(|c| c.version.clone()),
            size: child
                .as_ref()
                .filter(|c| c.r#type == FsType::File)
                .map(|c| c.size),
        });
        abort(signal, "list")?;
    }
    Ok(result)
}
fn not_text(verb: &str, path: &str) -> BoxError {
    FsError::new(
        format!("cannot {verb} \"{path}\": invalid UTF-8 text"),
        FsErrorCode::NotText,
    )
    .into()
}
fn decode(bytes: &[u8], verb: &str, path: &str) -> Result<String, BoxError> {
    let text = std::str::from_utf8(bytes).map_err(|_| not_text(verb, path))?;
    Ok(text.strip_prefix('\u{feff}').unwrap_or(text).into())
}
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
/// Node's `ERR_OUT_OF_RANGE` (`RangeError`) message for a numeric `received` value.
fn out_of_range(name: &str, range: &str, received: f64) -> BoxError {
    let text = hoh_util_values::js_number_string(received);
    let received = if received.fract() == 0.0 && received.abs() > 4_294_967_296.0 {
        // `addNumericalSeparator` works on the characters of `String(value)`.
        let chars: Vec<char> = text.chars().collect();
        let start = usize::from(chars.first() == Some(&'-'));
        let mut end = chars.len();
        let mut grouped = String::new();
        while end >= start + 4 {
            let part: String = chars[end - 3..end].iter().collect();
            grouped = format!("_{part}{grouped}");
            end -= 3;
        }
        format!("{}{grouped}", chars[..end].iter().collect::<String>())
    } else {
        text
    };
    Box::new(ErrnoError::new(
        Some("ERR_OUT_OF_RANGE".into()),
        format!("The value of \"{name}\" is out of range. It must be {range}. Received {received}"),
    ))
}
/// `validateInteger(value, name, 0)` of Node's `ReadStream` constructor.
fn validate_stream_integer(name: &str, value: f64) -> Result<(), BoxError> {
    if value > MAX_SAFE_INTEGER {
        return Err(out_of_range(name, ">= 0 && <= 9007199254740991", value));
    }
    Ok(())
}
async fn regular(
    target: &LocalTarget,
    signal: Option<&AbortSignal>,
) -> Result<std::fs::Metadata, BoxError> {
    abort(signal, "read")?;
    let m = fs::metadata(target.target_key.as_str())
        .await
        .map_err(|e| {
            fs_error_from_errno(
                "read",
                &target.display_path,
                errno(e, "stat", Some(target.target_key.as_str()), None),
            )
        })?;
    if !m.is_file() {
        return Err(FsError::new(
            format!(
                "cannot read \"{}\": not a regular file",
                target.display_path
            ),
            FsErrorCode::NotRegularFile,
        )
        .into());
    }
    Ok(m)
}
async fn read_raw(
    path: &str,
    display: &str,
    verb: &str,
    signal: Option<&AbortSignal>,
) -> Result<Vec<u8>, BoxError> {
    abort(signal, verb)?;
    let operation = async {
        let mut file = fs::File::open(path)
            .await
            .map_err(|e| errno(e, "open", Some(path), None))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .await
            .map_err(|e| errno(e, "read", None, None))?;
        Ok::<_, BoxError>(bytes)
    };
    tokio::pin!(operation);
    let result = if let Some(signal) = signal {
        tokio::select! {r=&mut operation=>r,_=signal.cancelled()=>return Err(FsError::new(format!("{verb} aborted"),FsErrorCode::Aborted).into())}
    } else {
        operation.await
    };
    result.map_err(|e| fs_error_from_errno(verb, display, e))
}
pub async fn read_whole_text(
    target: &LocalTarget,
    signal: Option<&AbortSignal>,
) -> Result<String, BoxError> {
    regular(target, signal).await?;
    let raw = read_raw(
        target.target_key.as_str(),
        &target.display_path,
        "read",
        signal,
    )
    .await?;
    abort(signal, "read")?;
    if raw.iter().take(BINARY_SAMPLE_BYTES).any(|b| *b == 0) {
        return Err(FsError::new(
            format!("cannot read \"{}\": binary file", target.display_path),
            FsErrorCode::NotText,
        )
        .into());
    }
    decode(&raw, "read", &target.display_path)
}
pub async fn read_whole_bytes(
    target: &LocalTarget,
    signal: Option<&AbortSignal>,
    max_bytes: u64,
    internals: &FsIoInternals,
) -> Result<Vec<u8>, BoxError> {
    let info = regular(target, signal).await?;
    if info.len() > max_bytes {
        let limit = hoh_util_values::js_number_string(max_bytes as f64);
        return Err(FsError::new(
            format!(
                "cannot read \"{}\": {} bytes exceeds the {limit}-byte limit",
                target.display_path,
                hoh_util_values::js_number_string(info.len() as f64)
            ),
            FsErrorCode::TooLarge,
        )
        .into());
    }
    if let Some(h) = &internals.inspect_read_bytes_after_stat {
        h(target.clone()).await?;
    }
    // `createReadStream({ end: maxBytes })` validates before opening the file.
    validate_stream_integer("end", max_bytes as f64)?;
    let mut file = fs::File::open(target.target_key.as_str())
        .await
        .map_err(|e| {
            fs_error_from_errno(
                "read",
                &target.display_path,
                errno(e, "open", Some(target.target_key.as_str()), None),
            )
        })?;
    let mut result = Vec::new();
    let mut buffer = [0; READ_CHUNK];
    loop {
        abort(signal, "read")?;
        let remaining = max_bytes
            .saturating_sub(result.len() as u64)
            .saturating_add(1);
        let count = remaining.min(READ_CHUNK as u64) as usize;
        let n = file.read(&mut buffer[..count]).await.map_err(|e| {
            fs_error_from_errno("read", &target.display_path, errno(e, "read", None, None))
        })?;
        if n == 0 {
            break;
        }
        result.extend_from_slice(&buffer[..n]);
        if result.len() as u64 > max_bytes {
            let limit = hoh_util_values::js_number_string(max_bytes as f64);
            return Err(FsError::new(
                format!(
                    "cannot read \"{}\": content exceeds the {limit}-byte limit",
                    target.display_path
                ),
                FsErrorCode::TooLarge,
            )
            .into());
        }
    }
    Ok(result)
}
pub async fn read_byte_window(
    target: &LocalTarget,
    range: FsByteRange,
    signal: Option<&AbortSignal>,
) -> Result<Vec<u8>, BoxError> {
    regular(target, signal).await?;
    if range.length == 0 {
        return Ok(vec![]);
    }
    // `createReadStream({ start, end })`: the JavaScript number arithmetic and checks, before open.
    let start = range.offset as f64;
    let end = start + range.length as f64 - 1.0;
    validate_stream_integer("start", start)?;
    validate_stream_integer("end", end)?;
    if start > end {
        return Err(out_of_range(
            "start",
            &format!(
                "<= \"end\" (here: {})",
                hoh_util_values::js_number_string(end)
            ),
            start,
        ));
    }
    let mut file = fs::File::open(target.target_key.as_str())
        .await
        .map_err(|e| {
            fs_error_from_errno(
                "read",
                &target.display_path,
                errno(e, "open", Some(target.target_key.as_str()), None),
            )
        })?;
    file.seek(std::io::SeekFrom::Start(range.offset))
        .await
        .map_err(|e| {
            fs_error_from_errno("read", &target.display_path, errno(e, "read", None, None))
        })?;
    let mut result = Vec::new();
    let mut buffer = [0; READ_CHUNK];
    while (result.len() as u64) < range.length {
        abort(signal, "read")?;
        let count = (range.length - result.len() as u64).min(READ_CHUNK as u64) as usize;
        let n = file.read(&mut buffer[..count]).await.map_err(|e| {
            fs_error_from_errno("read", &target.display_path, errno(e, "read", None, None))
        })?;
        if n == 0 {
            break;
        }
        result.extend_from_slice(&buffer[..n]);
    }
    Ok(result)
}
pub fn stream_whole_text(target: LocalTarget, signal: Option<AbortSignal>) -> FsTextStream {
    struct State {
        target: LocalTarget,
        signal: Option<AbortSignal>,
        file: Option<fs::File>,
        pending: Vec<u8>,
        sampled: usize,
        first: bool,
        finished: bool,
    }
    let state = State {
        target,
        signal,
        file: None,
        pending: vec![],
        sampled: 0,
        first: true,
        finished: false,
    };
    Box::pin(futures_util::stream::try_unfold(
        state,
        |mut state| async move {
            if state.finished {
                return Ok(None);
            }
            abort(state.signal.as_ref(), "read")?;
            if state.file.is_none() {
                regular(&state.target, state.signal.as_ref()).await?;
                state.file = Some(
                    fs::File::open(state.target.target_key.as_str())
                        .await
                        .map_err(|e| {
                            fs_error_from_errno(
                                "read",
                                &state.target.display_path,
                                errno(e, "open", Some(state.target.target_key.as_str()), None),
                            )
                        })?,
                );
            }
            let mut buffer = vec![0; READ_CHUNK];
            // Initialization guarantees the opened handle exists.
            let n = state
                .file
                .as_mut()
                .expect("stream handle opened")
                .read(&mut buffer)
                .await
                .map_err(|e| {
                    fs_error_from_errno(
                        "read",
                        &state.target.display_path,
                        errno(e, "read", None, None),
                    )
                })?;
            abort(state.signal.as_ref(), "read")?;
            buffer.truncate(n);
            let sample = (BINARY_SAMPLE_BYTES - state.sampled).min(n);
            if buffer[..sample].contains(&0) {
                return Err(FsError::new(
                    format!("cannot read \"{}\": binary file", state.target.display_path),
                    FsErrorCode::NotText,
                )
                .into());
            }
            state.sampled += sample;
            state.pending.extend(buffer);
            let complete = match std::str::from_utf8(&state.pending) {
                Ok(_) => state.pending.len(),
                Err(e) if e.error_len().is_none() && n != 0 => e.valid_up_to(),
                Err(_) => return Err(not_text("read", &state.target.display_path)),
            };
            let mut text = std::str::from_utf8(&state.pending[..complete])
                .map_err(|_| not_text("read", &state.target.display_path))?
                .to_owned();
            state.pending.drain(..complete);
            if state.first && !text.is_empty() {
                if let Some(no_bom) = text.strip_prefix('\u{feff}') {
                    text = no_bom.into();
                }
                state.first = false;
            }
            if n == 0 {
                state.finished = true;
                state.file = None;
            }
            Ok(Some((text, state)))
        },
    ))
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEndings {
    LF,
    CRLF,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditText {
    pub content: String,
    pub line_endings: LineEndings,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiteralEdit {
    pub content: String,
    pub replacements: u64,
}
pub fn normalize_line_endings(content: &str) -> String {
    content.replace("\r\n", "\n")
}
fn detect_line_endings(raw: &str) -> LineEndings {
    let units: Vec<_> = raw.encode_utf16().take(4096).collect();
    let crlf = units.windows(2).filter(|u| *u == [13, 10]).count();
    let lf = units.iter().filter(|u| **u == 10).count() - crlf;
    if crlf > lf {
        LineEndings::CRLF
    } else {
        LineEndings::LF
    }
}
pub fn restore_line_endings(content: &str, style: LineEndings) -> String {
    if style == LineEndings::LF {
        content.into()
    } else {
        normalize_line_endings(content).replace('\n', "\r\n")
    }
}
pub async fn read_for_edit(
    path: &str,
    display_path: &str,
    signal: Option<&AbortSignal>,
) -> Result<EditText, BoxError> {
    abort(signal, "edit")?;
    let bytes = read_raw(path, display_path, "edit", signal).await?;
    abort(signal, "edit")?;
    if bytes.contains(&0) {
        return Err(FsError::new(
            format!("cannot edit \"{display_path}\": binary file"),
            FsErrorCode::NotText,
        )
        .into());
    }
    let raw = decode(&bytes, "edit", display_path)?;
    Ok(EditText {
        line_endings: detect_line_endings(&raw),
        content: normalize_line_endings(&raw),
    })
}
pub async fn read_text_for_diff(
    path: &str,
    max_bytes: u64,
    signal: Option<&AbortSignal>,
) -> Result<Option<String>, BoxError> {
    read_text_for_diff_with_internals(path, max_bytes, signal, &FsIoInternals::default()).await
}
pub async fn read_text_for_diff_with_internals(
    path: &str,
    max_bytes: u64,
    signal: Option<&AbortSignal>,
    internals: &FsIoInternals,
) -> Result<Option<String>, BoxError> {
    abort(signal, "read")?;
    let result: Result<Option<String>, BoxError> = async {
        hook(internals, "open", path)?;
        let mut file = fs::File::open(path)
            .await
            .map_err(|e| errno(e, "open", Some(path), None))?;
        if let Some(h) = &internals.after_diff_open {
            h(path.into()).await?;
        }
        abort(signal, "read")?;
        let info = if let Some(h) = &internals.diff_metadata {
            h(path.into()).await?
        } else {
            let meta = file
                .metadata()
                .await
                .map_err(|e| errno(e, "fstat", None, None))?;
            path_info(&meta)
        };
        if let Some(h) = &internals.after_diff_stat {
            h(path.into()).await?;
        }
        abort(signal, "read")?;
        if info.r#type != FsType::File || info.size >= max_bytes {
            return Ok(None);
        }
        let capacity = usize::try_from(info.size)
            .ok()
            .and_then(|n| n.checked_add(1))
            .ok_or_else(|| FsError::new("file is too large to decode", FsErrorCode::TooLarge))?;
        if let Some(h) = &internals.inspect_diff_allocate {
            h(capacity);
        }
        let mut buffer = vec![0; capacity];
        let mut total = 0;
        while total < capacity {
            abort(signal, "read")?;
            let end = (total + READ_CHUNK).min(capacity);
            if let Some(h) = &internals.inspect_diff_read {
                h(end - total);
            }
            let n = file
                .read(&mut buffer[total..end])
                .await
                .map_err(|e| errno(e, "read", None, None))?;
            if let Some(h) = &internals.after_diff_read {
                h(path.into()).await?;
            }
            if n == 0 {
                break;
            }
            total += n;
        }
        drop(file);
        abort(signal, "read")?;
        if total as u64 != info.size || buffer[..total].contains(&0) {
            return Ok(None);
        }
        match decode(&buffer[..total], "read", path) {
            Ok(text) => Ok(Some(normalize_line_endings(&text))),
            Err(_) => Ok(None),
        }
    }
    .await;
    match result {
        Err(e) if HarnessError::of(e.as_ref()).is_some_and(|e| e.is_named(FsError::NAME)) => Err(e),
        Err(e) if code(e.as_ref()).is_some() => Ok(None),
        other => other,
    }
}
pub fn apply_literal_edit(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
    display_path: &str,
) -> Result<LiteralEdit, HarnessError> {
    let old = normalize_line_endings(old);
    if old.is_empty() {
        return Err(FsError::new(
            "old_string must be a non-empty string",
            FsErrorCode::EditNotFound,
        ));
    }
    let replacements = content.matches(&old).count() as u64;
    if replacements == 0 {
        return Err(FsError::new(
            format!("old_string was not found in \"{display_path}\""),
            FsErrorCode::EditNotFound,
        ));
    }
    if !replace_all && replacements > 1 {
        return Err(FsError::new(
            format!(
                "old_string matched {replacements} times in \"{display_path}\"; provide a more specific old_string or set replace_all to true"
            ),
            FsErrorCode::AmbiguousEdit,
        ));
    }
    Ok(LiteralEdit {
        content: content.replace(&old, &normalize_line_endings(new)),
        replacements,
    })
}

#[derive(Clone, Debug, Default)]
pub struct AtomicWriteOptions {
    pub display_path: Option<String>,
    pub create_if_absent: bool,
    pub existing_destination: Option<bool>,
}
fn write_io_error(path: &str, error: BoxError) -> BoxError {
    if HarnessError::of(error.as_ref()).is_some_and(|e| e.is_named(FsError::NAME)) {
        return error;
    }
    match code(error.as_ref()) {
        Some("EACCES" | "EPERM") => FsError::new(
            format!("cannot write \"{path}\": permission denied"),
            FsErrorCode::PermissionDenied,
        )
        .with_cause(error)
        .into(),
        Some(_) => FsError::new(
            format!("cannot write \"{path}\": {error}"),
            FsErrorCode::IoError,
        )
        .with_cause(error)
        .into(),
        None => error,
    }
}
fn uuid() -> Result<String, BoxError> {
    let mut b = [0; 16];
    getrandom::fill(&mut b).map_err(|e| std::io::Error::other(e.to_string()))?;
    b[6] = (b[6] & 15) | 64;
    b[8] = (b[8] & 63) | 128;
    let text: String = b.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &text[..8],
        &text[8..12],
        &text[12..16],
        &text[16..20],
        &text[20..]
    ))
}
async fn cleanup(path: &str, internals: &FsIoInternals) -> Result<(), BoxError> {
    if let Some(h) = &internals.remove_staging_dir {
        return h(path.into()).await;
    }
    match fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(errno(e, "rmdir", Some(path), None)),
    }
}
async fn guarded_create_error(
    original: BoxError,
    path: &str,
    display: &str,
    internals: &FsIoInternals,
) -> BoxError {
    let inspected = if let Some(h) = &internals.inspect_publication_target {
        h(path.into()).await
    } else {
        match fs::symlink_metadata(path).await {
            Ok(m) => Ok(Some(PathLinkInfo {
                version: version_of(&m),
                mode: m.mode() & 0o777,
                r#type: if m.file_type().is_symlink() {
                    FsPathType::Symlink
                } else {
                    path_type(&m).into()
                },
                size: m.len(),
            })),
            Err(e) if missing(&e) => Ok(None),
            Err(e) => Err(errno(e, "lstat", Some(path), None)),
        }
    };
    let existing = match inspected {
        Ok(info) => info,
        Err(e) if matches!(code(e.as_ref()), Some("ENOENT" | "ENOTDIR")) => None,
        Err(e) => {
            return FsError::new(
                format!("cannot write \"{display}\": {e}"),
                FsErrorCode::IoError,
            )
            .with_cause(e)
            .into();
        }
    };
    if let Some(existing) = existing {
        if existing.r#type != FsPathType::File {
            return FsError::new(
                format!("cannot write \"{display}\": not a regular file"),
                FsErrorCode::NotRegularFile,
            )
            .with_cause(original)
            .into();
        }
        return FsError::new(
            format!("cannot overwrite existing \"{display}\" without reading it first"),
            FsErrorCode::NotObserved,
        )
        .with_cause(original)
        .into();
    }
    if code(original.as_ref()) == Some("EEXIST") {
        return FsError::new(
            format!("cannot overwrite existing \"{display}\" without reading it first"),
            FsErrorCode::NotObserved,
        )
        .with_cause(original)
        .into();
    }
    FsError::new(
        format!("cannot write \"{display}\": {original}"),
        FsErrorCode::IoError,
    )
    .with_cause(original)
    .into()
}
pub async fn write_file_atomic(
    path: &str,
    content: &str,
    mode: Option<u32>,
    signal: Option<&AbortSignal>,
    internals: &FsIoInternals,
    options: AtomicWriteOptions,
) -> Result<(), BoxError> {
    let display = options.display_path.as_deref().unwrap_or(path);
    abort(signal, "write")?;
    let directory = dirname(path);
    fs::create_dir_all(&directory)
        .await
        .map_err(|e| write_io_error(display, errno(e, "mkdir", Some(&directory), None)))?;
    abort(signal, "write")?;
    let dir_name = if let Some(h) = &internals.temp_dir_name {
        h(path)?
    } else {
        format!(
            ".{}.{}.{}.tmpdir",
            basename(path),
            std::process::id(),
            uuid()?
        )
    };
    let staging_dir = join(&[&directory, &dir_name]);
    let temp_name = internals
        .temp_name
        .as_ref()
        .map(|h| h(path))
        .transpose()?
        .unwrap_or_else(|| format!("{}.tmp", basename(path)));
    let temp_path = join(&[&staging_dir, &temp_name]);
    let mut staging_created = false;
    let mut handle: Option<fs::File> = None;
    // A failed first close still owns the `handle` in the source; the cleanup close runs again.
    let mut close_retry = false;
    let result = async {
        hook(internals, "mkdir", &staging_dir)?;
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder
            .create(&staging_dir)
            .await
            .map_err(|e| errno(e, "mkdir", Some(&staging_dir), None))?;
        staging_created = true;
        fs::set_permissions(&staging_dir, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(|e| errno(e, "chmod", Some(&staging_dir), None))?;
        let file = handle.insert(
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temp_path)
                .await
                .map_err(|e| errno(e, "open", Some(&temp_path), None))?,
        );
        let staged = async {
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .await
                .map_err(|e| errno(e, "fchmod", None, None))?;
            for chunk in content.as_bytes().chunks(READ_CHUNK) {
                abort(signal, "write")?;
                file.write_all(chunk)
                    .await
                    .map_err(|e| errno(e, "write", None, None))?;
                file.flush()
                    .await
                    .map_err(|e| errno(e, "write", None, None))?;
            }
            // Observe queued write errors before Tokio sync_all can swallow them.
            file.flush()
                .await
                .map_err(|e| errno(e, "write", None, None))?;
            file.sync_all()
                .await
                .map_err(|e| errno(e, "fsync", None, None))?;
            if let Some(h) = &internals.inspect_temp {
                h(StagedPaths {
                    staging_dir: staging_dir.clone(),
                    temp_path: temp_path.clone(),
                })
                .await?;
            }
            if let Some(mode) = mode {
                file.set_permissions(std::fs::Permissions::from_mode(mode))
                    .await
                    .map_err(|e| errno(e, "fchmod", None, None))?;
            }
            Ok::<_, BoxError>(())
        }
        .await;
        staged?;
        // fsio.ts:648: a failed close() rejects before anything is published.
        if let Err(e) = close_attempt(handle.take(), internals).await {
            close_retry = true;
            return Err(e);
        }
        abort(signal, "write")?;
        if options.create_if_absent {
            let published = if let Some(h) = &internals.link_file {
                h(temp_path.clone(), path.into()).await
            } else {
                fs::hard_link(&temp_path, path)
                    .await
                    .map_err(|e| errno(e, "link", Some(&temp_path), Some(path)))
            };
            if let Err(e) = published {
                return Err(guarded_create_error(e, path, display, internals).await);
            }
        } else {
            hook(internals, "rename", path)?;
            fs::rename(&temp_path, path)
                .await
                .map_err(|e| errno(e, "rename", Some(&temp_path), Some(path)))?;
        }
        // Publication is the commit point; cleanup residue cannot undo success.
        let _ = cleanup(&staging_dir, internals).await;
        Ok(())
    }
    .await;
    match result {
        Ok(()) => Ok(()),
        Err(e) => {
            let failure = if hoh_timeout::is_abort_error(e.as_ref())
                || HarnessError::of(e.as_ref()).is_some_and(|e| e.name() == "AbortError")
            {
                FsError::new("write aborted", FsErrorCode::Aborted).into()
            } else {
                write_io_error(display, e)
            };
            let failure = {
                let leftover = handle.take();
                if leftover.is_some() || close_retry {
                    match close_attempt(leftover, internals).await {
                        Ok(()) => failure,
                        Err(close_error) => FsError::new(
                            format!(
                                "write failed ({failure}) and temp close failed ({close_error})"
                            ),
                            FsErrorCode::NotFound,
                        )
                        .with_cause(failure)
                        .into(),
                    }
                } else {
                    failure
                }
            };
            if !staging_created {
                return Err(failure);
            }
            if let Err(cleanup_error) = cleanup(&staging_dir, internals).await {
                return Err(FsError::new(
                    format!("write failed ({failure}) and temp cleanup failed ({cleanup_error})"),
                    FsErrorCode::NotFound,
                )
                .with_cause(failure)
                .into());
            }
            Err(failure)
        }
    }
}
fn directory_remove_error(path: &str) -> BoxError {
    Box::new(ErrnoError::new(
        Some("ERR_FS_EISDIR".into()),
        format!("Path is a directory: rm returned EISDIR (is a directory) {path}"),
    ))
}
async fn remove_native(path: &str) -> Result<(), BoxError> {
    // Node rm validates with lstat before unlink, including missing/denied paths.
    let info = fs::symlink_metadata(path)
        .await
        .map_err(|e| errno(e, "lstat", Some(path), None))?;
    if info.is_dir() {
        return Err(directory_remove_error(path));
    }
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::IsADirectory => Err(directory_remove_error(path)),
        Err(e) => Err(errno(e, "unlink", Some(path), None)),
    }
}
pub async fn remove_file(
    path: &str,
    display: &str,
    internals: &FsIoInternals,
) -> Result<(), BoxError> {
    let result = if let Some(h) = &internals.remove_file {
        h(path.into()).await
    } else {
        remove_native(path).await
    };
    result.map_err(|e| {
        let kind = if matches!(code(e.as_ref()), Some("EACCES" | "EPERM")) {
            FsErrorCode::PermissionDenied
        } else {
            FsErrorCode::IoError
        };
        Box::new(FsError::new(format!("cannot delete \"{display}\": {e}"), kind).with_cause(e))
            as BoxError
    })
}
