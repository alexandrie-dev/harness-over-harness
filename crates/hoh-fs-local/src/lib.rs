//! Host filesystem provider with physical identity and atomic mutations (Linux).
pub mod fsio;
mod locks;
mod shim;
pub mod watch;
use fsio::{
    AtomicWriteOptions, FsIoInternals, PathInfo, normalize_line_endings, restore_line_endings,
};
use hoh_error::{BoxError, HarnessError};
use hoh_fs::*;
use hoh_fs_patch::{ApplyHunksResult, PatchHunk, apply_hunks, types::FailureStage};
use hoh_kernel::{Cfg, Context, plugin};
use hoh_timeout::AbortSignal;
use hoh_util_values::js_trim;
use hoh_util_workspace_path::node_path;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, MutexGuard};

pub const DEFAULT_DIFF_BASIS_MAX_BYTES: u64 = 10 * 1024 * 1024;
pub const MAX_DIFF_BASIS_BYTES: u64 = 536_870_888;
fn default_limit() -> f64 {
    DEFAULT_DIFF_BASIS_MAX_BYTES as f64
}
fn schema_cwd() -> Option<String> {
    std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}
fn deserialize_cwd<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    String::deserialize(d).map(Some)
}
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Base directory for relative paths. Defaults to `process.cwd()`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_cwd"
    )]
    #[schemars(with = "String")]
    #[schemars(extend("default"=schema_cwd()))]
    pub cwd: Option<String>,
    /// Exclusive UTF-8 byte limit on each overwrite-diff side, capped by the
    /// runtime's safe allocation/decode maximum. Defaults to 10 MiB.
    #[serde(default = "default_limit")]
    pub diff_basis_max_bytes: f64,
}
impl<'de> Deserialize<'de> for Config {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Input {
            #[serde(default, deserialize_with = "deserialize_cwd")]
            cwd: Option<String>,
            #[serde(default = "default_limit")]
            diff_basis_max_bytes: f64,
        }
        let input = Input::deserialize(d)?;
        let cwd = match input.cwd {
            Some(cwd) => cwd,
            None => std::env::current_dir()
                .map_err(|e| serde::de::Error::custom(fsio::errno(e, "uv_cwd", None, None)))?
                .to_string_lossy()
                .into_owned(),
        };
        Ok(Self {
            cwd: Some(cwd),
            diff_basis_max_bytes: input.diff_basis_max_bytes,
        })
    }
}
impl Default for Config {
    fn default() -> Self {
        Self {
            cwd: None,
            diff_basis_max_bytes: default_limit(),
        }
    }
}
#[derive(Clone, Debug)]
pub struct ResolvedConfig {
    pub cwd: String,
    pub diff_basis_max_bytes: u64,
}
#[derive(Clone)]
pub struct LocalFileSystem {
    pub config: Arc<ResolvedConfig>,
    pub internals: Arc<Mutex<FsIoInternals>>,
    pub watch_factory: Arc<Mutex<Option<Arc<dyn watch::WatchFactory>>>>,
    locks: Arc<locks::Locks>,
}
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}
impl LocalFileSystem {
    pub fn new(config: Config) -> Result<Self, BoxError> {
        let n = config.diff_basis_max_bytes;
        if !n.is_finite() || n.fract() != 0.0 || n <= 0.0 || n > MAX_DIFF_BASIS_BYTES as f64 {
            return Err(std::io::Error::other(format!("fs-local: diffBasisMaxBytes must be a positive safe integer no greater than {MAX_DIFF_BASIS_BYTES}")).into());
        }
        let cwd = match config.cwd {
            Some(cwd) => cwd,
            None => std::env::current_dir()
                .map_err(|e| fsio::errno(e, "uv_cwd", None, None))?
                .to_string_lossy()
                .into_owned(),
        };
        Ok(Self {
            config: Arc::new(ResolvedConfig {
                cwd,
                diff_basis_max_bytes: n as u64,
            }),
            internals: Arc::default(),
            watch_factory: Arc::default(),
            locks: Arc::default(),
        })
    }
    fn hooks(&self) -> FsIoInternals {
        lock(&self.internals).clone()
    }
    #[doc(hidden)]
    pub fn mutation_lock_count(&self) -> usize {
        self.locks.len()
    }
    async fn write_target(
        &self,
        target: &FsTarget,
        expected: Option<&FsWriteIntent>,
    ) -> Result<Option<PathInfo>, BoxError> {
        let existing = fsio::probe(target.target_key.as_str()).await?;
        if existing.as_ref().is_some_and(|i| i.r#type != FsType::File) {
            return Err(FsError::new(
                format!(
                    "cannot write \"{}\": not a regular file",
                    target.display_path
                ),
                FsErrorCode::NotRegularFile,
            )
            .into());
        }
        match expected {
            Some(FsWriteIntent::ReplaceIfVersion { version }) => {
                let info = existing.as_ref().ok_or_else(|| {
                    FsError::new(
                        format!(
                            "cannot write \"{}\": file no longer exists",
                            target.display_path
                        ),
                        FsErrorCode::StaleVersion,
                    )
                })?;
                if &info.version != version {
                    return Err(FsError::new(
                        format!(
                            "cannot write \"{}\": file changed since it was read",
                            target.display_path
                        ),
                        FsErrorCode::StaleVersion,
                    )
                    .into());
                }
            }
            Some(FsWriteIntent::CreateIfAbsent) if existing.is_some() => {
                return Err(FsError::new(
                    format!(
                        "cannot overwrite existing \"{}\" without reading it first",
                        target.display_path
                    ),
                    FsErrorCode::NotObserved,
                )
                .into());
            }
            _ => {}
        }
        Ok(existing)
    }
    async fn require_file(
        &self,
        target: &FsTarget,
        expected: Option<&FsVersionGuard>,
        verb: &str,
    ) -> Result<PathInfo, BoxError> {
        let info = fsio::probe(target.target_key.as_str())
            .await?
            .ok_or_else(|| {
                FsError::new(
                    format!(
                        "cannot {verb} \"{}\": file changed since it was read",
                        target.display_path
                    ),
                    FsErrorCode::StaleVersion,
                )
            })?;
        if info.r#type != FsType::File {
            return Err(FsError::new(
                format!(
                    "cannot {verb} \"{}\": not a regular file",
                    target.display_path
                ),
                FsErrorCode::NotRegularFile,
            )
            .into());
        }
        if expected.is_some_and(|e| e.version != info.version) {
            return Err(FsError::new(
                format!(
                    "cannot {verb} \"{}\": file changed since it was read",
                    target.display_path
                ),
                FsErrorCode::StaleVersion,
            )
            .into());
        }
        Ok(info)
    }
    fn patch_content(
        target: &FsTarget,
        before: &str,
        hunks: &[PatchHunk],
    ) -> Result<String, BoxError> {
        match apply_hunks(before, hunks) {
            ApplyHunksResult::Success { after } => Ok(after),
            ApplyHunksResult::Failure { failure } => {
                let stage = match failure.stage {
                    FailureStage::Context => "context",
                    FailureStage::OldLines => "oldLines",
                };
                Err(FsError::new(
                    format!(
                        "cannot patch \"{}\": hunk at patch line {} ({stage}) does not match",
                        target.display_path, failure.hunk_line_number
                    ),
                    FsErrorCode::PatchMismatch,
                )
                .into())
            }
        }
    }
    async fn version_after_write(target: &FsTarget) -> Result<FsVersion, BoxError> {
        Ok(fsio::probe(target.target_key.as_str())
            .await?
            .map(|i| i.version)
            .unwrap_or_else(|| format!("missing:{}", target.target_key.as_str()).into()))
    }
    async fn remove(&self, target: &FsTarget, hooks: &FsIoInternals) -> Result<(), BoxError> {
        if let Some(h) = &hooks.remove_file_override {
            h(
                target.target_key.as_str().into(),
                target.display_path.clone(),
            )
            .await
        } else {
            fsio::remove_file(target.target_key.as_str(), &target.display_path, hooks).await
        }
    }
}
impl FileSystem for LocalFileSystem {
    fn watch(
        &self,
        target: &FsTarget,
        changed: FsChanged,
        signal: AbortSignal,
    ) -> FsFuture<'_, Arc<dyn FsWatcher>> {
        let fs = self.clone();
        let target = target.clone();
        Box::pin(async move { watch::watch_target(&fs, target, changed, signal).await })
    }
    fn resolve<'a>(&'a self, path: &'a str, options: FsResolveOptions) -> FsFuture<'a, FsTarget> {
        Box::pin(async move {
            fsio::abort(options.signal.as_ref(), "resolve")?;
            let hooks = self.hooks();
            let path =
                fsio::expand_home_prefix_with_override(path, hooks.home_directory.as_deref())?;
            let target = fsio::resolve_local_target_with_internals(
                options.cwd.as_deref().unwrap_or(&self.config.cwd),
                &path,
                &hooks,
            )
            .await?;
            fsio::abort(options.signal.as_ref(), "resolve")?;
            Ok(target)
        })
    }
    fn process_path(&self, target: &FsTarget) -> String {
        target.target_key.as_str().into()
    }
    fn process_path_from_host_path(&self, path: &str) -> Option<String> {
        node_path::is_absolute(path).then(|| node_path::resolve("/", &[path]))
    }
    fn file_url(&self, target: &FsTarget) -> String {
        url::Url::from_file_path(target.target_key.as_str())
            .map(|u| u.to_string())
            .unwrap_or_else(|_| format!("file://{}", target.target_key.as_str()))
    }
    fn contains(&self, parent: &FsTarget, child: &FsTarget) -> bool {
        let p = node_path::relative("/", parent.target_key.as_str(), child.target_key.as_str());
        p.is_empty() || (p != ".." && !p.starts_with("../") && !node_path::is_absolute(&p))
    }
    fn stat<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Option<FsInfo>> {
        Box::pin(async move {
            fsio::abort(signal.as_ref(), "stat")?;
            let hooks = self.hooks();
            let info = fsio::probe_with_internals(target.target_key.as_str(), &hooks).await?;
            if let Some(h) = &hooks.after_metadata {
                h("stat".into(), target.target_key.as_str().into()).await?;
            }
            fsio::abort(signal.as_ref(), "stat")?;
            Ok(info.map(|i| FsInfo {
                version: i.version,
                r#type: i.r#type,
                size: Some(i.size),
            }))
        })
    }
    fn lstat<'a>(
        &'a self,
        path: &'a str,
        cwd: Option<&'a str>,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Option<FsPathInfo>> {
        Box::pin(async move {
            fsio::abort(signal.as_ref(), "lstat")?;
            if js_trim(path).is_empty() {
                return Err(FsError::new(
                    "file_path must be a non-empty string",
                    FsErrorCode::NotFound,
                )
                .into());
            }
            let hooks = self.hooks();
            let path = fsio::local_display_path(
                cwd.unwrap_or(&self.config.cwd),
                &fsio::expand_home_prefix_with_override(path, hooks.home_directory.as_deref())?,
            )?;
            let info = fsio::probe_no_follow(&path).await?;
            if let Some(h) = &hooks.after_metadata {
                h("lstat".into(), path).await?;
            }
            fsio::abort(signal.as_ref(), "lstat")?;
            Ok(info.map(|i| FsPathInfo {
                version: i.version,
                r#type: i.r#type,
                size: Some(i.size),
            }))
        })
    }
    fn read_text<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, String> {
        Box::pin(async move { fsio::read_whole_text(target, signal.as_ref()).await })
    }
    fn stream_text<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, FsTextStream> {
        Box::pin(std::future::ready(Ok(fsio::stream_whole_text(
            target.clone(),
            signal,
        ))))
    }
    fn read_bytes<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
        max: u64,
    ) -> FsFuture<'a, Vec<u8>> {
        Box::pin(async move {
            fsio::read_whole_bytes(target, signal.as_ref(), max, &self.hooks()).await
        })
    }
    fn read_byte_range<'a>(
        &'a self,
        target: &'a FsTarget,
        range: FsByteRange,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Vec<u8>> {
        Box::pin(async move { fsio::read_byte_window(target, range, signal.as_ref()).await })
    }
    fn list_dir<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Vec<FsDirEntry>> {
        Box::pin(async move {
            Ok(
                fsio::list_directory_with_internals(target, signal.as_ref(), &self.hooks())
                    .await?
                    .into_iter()
                    .map(|e| FsDirEntry {
                        name: e.name,
                        r#type: e.r#type,
                        target: e.target,
                        version: e.version,
                        size: e.size,
                    })
                    .collect(),
            )
        })
    }
    fn write_text<'a>(
        &'a self,
        target: &'a FsTarget,
        content: &'a str,
        expected: Option<FsWriteIntent>,
        signal: Option<AbortSignal>,
        _policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsWriteOutcome> {
        let fs = self.clone();
        let target = target.clone();
        let content = content.to_owned();
        self.locks
            .run(vec![target.target_key.as_str().into()], async move {
                let existing = fs.write_target(&target, expected.as_ref()).await?;
                let hooks = fs.hooks();
                let before = if existing.is_some()
                    && (content.len() as u64) < fs.config.diff_basis_max_bytes
                {
                    fsio::read_text_for_diff_with_internals(
                        target.target_key.as_str(),
                        fs.config.diff_basis_max_bytes,
                        signal.as_ref(),
                        &hooks,
                    )
                    .await?
                } else {
                    None
                };
                fsio::write_file_atomic(
                    target.target_key.as_str(),
                    &content,
                    existing.as_ref().map(|i| i.mode),
                    signal.as_ref(),
                    &hooks,
                    AtomicWriteOptions {
                        display_path: Some(target.display_path.clone()),
                        create_if_absent: matches!(expected, Some(FsWriteIntent::CreateIfAbsent)),
                        existing_destination: None,
                    },
                )
                .await?;
                Ok(FsWriteOutcome {
                    operation: if existing.is_some() {
                        FsWriteOperation::Update
                    } else {
                        FsWriteOperation::Create
                    },
                    version: Self::version_after_write(&target).await?,
                    before,
                    after: normalize_line_endings(&content),
                })
            })
    }
    fn edit_text<'a>(
        &'a self,
        target: &'a FsTarget,
        edit: FsEditRequest,
        expected: Option<FsVersionGuard>,
        signal: Option<AbortSignal>,
        _policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsEditOutcome> {
        let fs = self.clone();
        let target = target.clone();
        self.locks
            .run(vec![target.target_key.as_str().into()], async move {
                let info = fs.require_file(&target, expected.as_ref(), "edit").await?;
                let original = fsio::read_for_edit(
                    target.target_key.as_str(),
                    &target.display_path,
                    signal.as_ref(),
                )
                .await?;
                let edited = fsio::apply_literal_edit(
                    &original.content,
                    &edit.old_string,
                    &edit.new_string,
                    edit.replace_all,
                    &target.display_path,
                )?;
                fsio::write_file_atomic(
                    target.target_key.as_str(),
                    &restore_line_endings(&edited.content, original.line_endings),
                    Some(info.mode),
                    signal.as_ref(),
                    &fs.hooks(),
                    AtomicWriteOptions {
                        display_path: Some(target.display_path.clone()),
                        ..Default::default()
                    },
                )
                .await?;
                Ok(FsEditOutcome {
                    version: Self::version_after_write(&target).await?,
                    before: original.content,
                    after: edited.content,
                })
            })
    }
    fn patch_text<'a>(
        &'a self,
        target: &'a FsTarget,
        hunks: &'a [PatchHunk],
        expected: Option<FsVersionGuard>,
        signal: Option<AbortSignal>,
        _policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsEditOutcome> {
        let fs = self.clone();
        let target = target.clone();
        let hunks = hunks.to_vec();
        self.locks
            .run(vec![target.target_key.as_str().into()], async move {
                let info = fs.require_file(&target, expected.as_ref(), "patch").await?;
                let original = fsio::read_for_edit(
                    target.target_key.as_str(),
                    &target.display_path,
                    signal.as_ref(),
                )
                .await?;
                let after = Self::patch_content(&target, &original.content, &hunks)?;
                fsio::write_file_atomic(
                    target.target_key.as_str(),
                    &restore_line_endings(&after, original.line_endings),
                    Some(info.mode),
                    signal.as_ref(),
                    &fs.hooks(),
                    AtomicWriteOptions {
                        display_path: Some(target.display_path.clone()),
                        ..Default::default()
                    },
                )
                .await?;
                Ok(FsEditOutcome {
                    version: Self::version_after_write(&target).await?,
                    before: original.content,
                    after,
                })
            })
    }
    fn delete_file<'a>(
        &'a self,
        target: &'a FsTarget,
        expected: Option<FsVersionGuard>,
        signal: Option<AbortSignal>,
        _policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsDeleteOutcome> {
        let fs = self.clone();
        let target = target.clone();
        self.locks
            .run(vec![target.target_key.as_str().into()], async move {
                fs.require_file(&target, expected.as_ref(), "delete")
                    .await?;
                let hooks = fs.hooks();
                let before = fsio::read_text_for_diff_with_internals(
                    target.target_key.as_str(),
                    fs.config.diff_basis_max_bytes,
                    signal.as_ref(),
                    &hooks,
                )
                .await?;
                if let Some(h) = &hooks.inspect_before_mutation {
                    h(target.clone()).await?;
                }
                fsio::abort(signal.as_ref(), "delete")?;
                fs.remove(&target, &hooks).await?;
                Ok(FsDeleteOutcome { before })
            })
    }
    fn move_file<'a>(
        &'a self,
        source: &'a FsTarget,
        destination: &'a FsTarget,
        options: FsMoveOptions,
        signal: Option<AbortSignal>,
        _policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsMoveOutcome> {
        if source.target_key == destination.target_key {
            return Box::pin(std::future::ready(Err(FsError::new(
                format!("cannot move \"{}\" to itself", source.display_path),
                FsErrorCode::SameTarget,
            )
            .into())));
        }
        let fs = self.clone();
        let source = source.clone();
        let destination = destination.clone();
        self.locks.run(
            vec![
                source.target_key.as_str().into(),
                destination.target_key.as_str().into(),
            ],
            async move {
                let source_info = fs
                    .require_file(&source, options.expected_source.as_ref(), "move")
                    .await?;
                let existing = fs
                    .write_target(&destination, options.expected_destination.as_ref())
                    .await?;
                let original = fsio::read_for_edit(
                    source.target_key.as_str(),
                    &source.display_path,
                    signal.as_ref(),
                )
                .await?;
                let after = if let Some(hunks) = options.hunks {
                    Self::patch_content(&source, &original.content, &hunks)?
                } else {
                    original.content.clone()
                };
                let content = restore_line_endings(&after, original.line_endings);
                let hooks = fs.hooks();
                let before = if existing.is_some()
                    && (content.len() as u64) < fs.config.diff_basis_max_bytes
                {
                    fsio::read_text_for_diff_with_internals(
                        destination.target_key.as_str(),
                        fs.config.diff_basis_max_bytes,
                        signal.as_ref(),
                        &hooks,
                    )
                    .await?
                } else {
                    None
                };
                if let Some(h) = &hooks.inspect_before_mutation {
                    h(source.clone()).await?;
                }
                fsio::abort(signal.as_ref(), "move")?;
                fsio::write_file_atomic(
                    destination.target_key.as_str(),
                    &content,
                    Some(source_info.mode),
                    None,
                    &hooks,
                    AtomicWriteOptions {
                        display_path: Some(destination.display_path.clone()),
                        create_if_absent: matches!(
                            options.expected_destination,
                            Some(FsWriteIntent::CreateIfAbsent)
                        ),
                        existing_destination: Some(existing.is_some()),
                    },
                )
                .await?;
                let destination = FsWriteOutcome {
                    operation: if existing.is_some() {
                        FsWriteOperation::Update
                    } else {
                        FsWriteOperation::Create
                    },
                    version: Self::version_after_write(&destination).await?,
                    before,
                    after,
                };
                let source_before = ((original.content.len() as u64)
                    < fs.config.diff_basis_max_bytes)
                    .then_some(original.content);
                match fs.remove(&source, &hooks).await {
                    Ok(()) => Ok(FsMoveOutcome::Deleted {
                        source_before,
                        destination,
                    }),
                    Err(error) => {
                        if let Some(typed) =
                            HarnessError::of(error.as_ref()).filter(|e| e.is_named(FsError::NAME))
                        {
                            let code = FsErrorCode::from_code(typed.code())
                                .ok_or_else(|| std::io::Error::other("unknown FsError code"))?;
                            Ok(FsMoveOutcome::Retained {
                                source_before,
                                destination,
                                source_failure: FsMoveSourceFailure {
                                    code,
                                    message: typed.message().into(),
                                },
                            })
                        } else {
                            Err(error)
                        }
                    }
                }
            },
        )
    }
}
#[plugin(FsLocalPlugin, name = "LocalFileSystem")]
pub async fn apply(ctx: Context, config: Cfg<Config>) -> Result<(), BoxError> {
    let provider = Arc::new(LocalFileSystem::new((*config.get()).clone())?);
    ctx.provide::<Fs>(provider).await?;
    Ok(())
}
