//! Port of @deepseek-ai/dsh-credentials-local (packages/credentials/credentials-local).
//!
//! File-backed credentials provider over `$DSH_HOME/.credentials.yaml`, layered over the
//! environment in this precedence order:
//!
//! ```text
//! $DSH_HOME/.credentials.yaml        (provider-managed, written by set/unset)
//! > inherited process environment    (fallback; this provider never writes it)
//! > <invocation cwd>/.env            (fallback; this provider never writes it)
//! > $DSH_HOME/.env                   (fallback; this provider never writes it)
//! ```
//!
//! The managed store wins because a credential the user explicitly saved in the product must take
//! effect consistently for every provider. The file is the provider-managed writable source:
//! every write re-reads the document under a cross-process writer lock before patching only its
//! own entry, external edits hot-publish through the seam, and each reload replaces the snapshot
//! wholesale so a deleted entry never lingers in memory.
//!
//! The document holds credentials: a credential value never reaches a log line, a `Debug`
//! rendering or an error message of this crate; diagnostics carry key names and positions only.

mod document;
mod edit;
mod emit;
mod syntax;
pub mod watch;

use std::{
    future::Future,
    io,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc, Mutex, MutexGuard, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use hoh_atomic_write::{
    FileLockOptions, WriteFileAtomicOptions, with_file_lock, write_file_atomic,
};
use hoh_credentials::{
    CredentialFuture, CredentialInfo, CredentialKey, CredentialMutator, CredentialProvider,
    CredentialRecord, CredentialRecordEntry, CredentialRecordInfo, CredentialRef, Credentials,
    ResolvedCredential, credential_ref, parse_credential_key,
};
use hoh_error::{BoxError, node_fs_error_message};
use hoh_home_paths::{canonicalize_watch_path, resolve_dsh_home};
use hoh_kernel::{Cfg, Context, Plugin, Undo};
use hoh_launch_environment::{LaunchEnvironmentSource, launch_environment_of};
use hoh_util_values::js_json_stringify;
use hoh_util_workspace_path::node_path;
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use document::{
    CredentialsDocument, DOCUMENT_VERSION, parse_credentials_document, render_flat_layout_migration,
};
use document::{assert_storable_api_key, mask_credential, same_record};
use watch::{WatchRequest, WatchSink, WatcherFactory};

/// Basename of the credentials document inside the harness home.
pub const CREDENTIALS_FILENAME: &str = ".credentials.yaml";

/// Plugin config: file location and hot-reload behavior.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Credentials document path; defaults to `.credentials.yaml` under the harness home.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Harness home used when `path` is omitted; defaults to `$DSH_HOME` or `~/.dsh`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dsh_home: Option<String>,
    /// Watch the document and hot-publish external edits; defaults to true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch: Option<bool>,
    /// Watcher write-settle window in milliseconds; defaults to 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debounce_ms: Option<f64>,
}

/// Fully resolved provider parameters; defaulting happens here, never inline.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedSpec {
    pub filename: PathBuf,
    pub watch: bool,
    pub debounce_ms: f64,
}

/// Resolve the runtime spec from plugin config: an explicit `path` wins, otherwise the document
/// lives at `<harness home>/.credentials.yaml`.
pub fn resolve_spec(config: &Config) -> Result<ResolvedSpec, BoxError> {
    let raw = match &config.path {
        Some(path) => path.clone(),
        None => {
            let home = resolve_dsh_home(config.dsh_home.as_deref(), None)?;
            node_path::join(&[&home.to_string_lossy(), CREDENTIALS_FILENAME])
        }
    };
    let cwd = std::env::current_dir()?;
    Ok(ResolvedSpec {
        filename: PathBuf::from(node_path::resolve(&cwd.to_string_lossy(), &[&raw])),
        watch: config.watch.unwrap_or(true),
        debounce_ms: config.debounce_ms.unwrap_or(100.0),
    })
}

/// Permission bits outside the owner; a credentials document must have none of them.
#[cfg(unix)]
const GROUP_OTHER_BITS: u32 = 0o077;

/// How long a record write waits for the cross-process writer lock. A record mutation runs its
/// caller's decision while holding the lock, and for the operation this half exists to serve — an
/// owner refreshing an expired token — that decision includes a network round trip. Refs and
/// records share one file and one lock, so every writer of this document waits this long.
const DOCUMENT_LOCK_WAIT_MS: u64 = 30_000;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Reject a credentials document other OS users can read, before its contents are read at all.
///
/// POSIX only: Windows has no mode to inspect, so the check is skipped rather than faked.
async fn assert_owner_only(filename: &Path) -> Result<(), BoxError> {
    let display = filename.to_string_lossy();
    let metadata = match tokio::fs::metadata(filename).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            canonicalize_watch_path(filename)
                .await
                .map_err(|error| -> BoxError {
                    node_fs_error_message(&error, "stat", Some(&display), None).into()
                })?;
            return Ok(());
        }
        Err(error) => {
            return Err(node_fs_error_message(&error, "stat", Some(&display), None).into());
        }
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode();
        if mode & GROUP_OTHER_BITS != 0 {
            return Err(format!(
                "credentials-local: {display} is readable beyond its owner (mode {:o}); run \"chmod 600 {display}\" before starting again",
                mode & 0o777
            )
            .into());
        }
    }
    #[cfg(not(unix))]
    let _ = metadata;
    Ok(())
}

/// Read the document; `None` while the file is absent, every other failure surfaces.
async fn read_document(filename: &Path) -> Result<Option<String>, BoxError> {
    match tokio::fs::read(filename).await {
        // `readFile(..., 'utf8')` decodes invalid bytes to U+FFFD.
        Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(
                node_fs_error_message(&error, "read", Some(&filename.to_string_lossy()), None)
                    .into(),
            )
        }
    }
}

/// `mkdir(dirname, { recursive: true, mode: 0o700 })`: the harness home holds user-private data.
async fn ensure_directory(filename: &Path) -> Result<(), BoxError> {
    let Some(directory) = filename.parent() else {
        return Ok(());
    };
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder
        .create(directory)
        .await
        .map_err(|error| -> BoxError {
            node_fs_error_message(&error, "mkdir", Some(&directory.to_string_lossy()), None).into()
        })?;
    // `mode` only applies to directories this call creates. One that already exists with group or
    // other access is refused rather than silently re-permissioned: the path may be a shared
    // directory the provider has no business changing, and writing secrets under it is the
    // caller's decision to make by running the `chmod` named here.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = tokio::fs::metadata(directory)
            .await
            .map_err(|error| -> BoxError {
                node_fs_error_message(&error, "stat", Some(&directory.to_string_lossy()), None)
                    .into()
            })?;
        let mode = metadata.permissions().mode();
        if mode & GROUP_OTHER_BITS != 0 {
            let display = directory.to_string_lossy();
            return Err(format!(
                "credentials-local: directory {display} is accessible beyond its owner (mode {:o}); run \"chmod 700 {display}\" before storing credentials",
                mode & 0o777
            )
            .into());
        }
    }
    Ok(())
}

/// Atomic, owner-only replacement of the document (`writeFileAtomic` at 0600 in a 0700 directory).
pub type DocumentWriter = Arc<
    dyn Fn(PathBuf, String) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send>> + Send + Sync,
>;

fn default_writer() -> DocumentWriter {
    Arc::new(|path, content| {
        Box::pin(async move {
            write_file_atomic(
                path,
                content,
                WriteFileAtomicOptions {
                    mode: 0o600,
                    dir_mode: Some(0o700),
                },
            )
            .await
        })
    })
}

/// The two operating-system boundaries the provider crosses, replaceable for tests: the atomic
/// document write and the file watcher.
#[doc(hidden)]
#[derive(Clone, Default)]
pub struct Seams {
    pub writer: Option<DocumentWriter>,
    pub watcher: Option<WatcherFactory>,
}

#[derive(Default)]
struct State {
    /// Raw text of the last read or persisted document; `None` while the file is absent. Watcher
    /// events whose content equals this cache are no-ops, which is also the self-write suppression.
    text: Option<String>,
    /// Parsed reference snapshot; replaced wholesale on every reload.
    values: IndexMap<String, String>,
    /// Parsed record snapshot; replaced wholesale on every reload.
    records: IndexMap<String, CredentialRecord>,
}

/// File-backed credentials provider (`$DSH_HOME/.credentials.yaml`).
pub struct LocalCredentialProvider {
    ctx: Context,
    spec: ResolvedSpec,
    state: Mutex<State>,
    /// The single exclusive operation chain: watcher reloads and document edits run one at a time
    /// in queue order, so an edit can never render from text a concurrent reload is replacing.
    operations: tokio::sync::Mutex<()>,
    /// Set at dispose: refuse new writes and let in-flight work no-op.
    closed: AtomicBool,
    writer: DocumentWriter,
    me: Weak<Self>,
}

impl std::fmt::Debug for LocalCredentialProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the stored values: only where the document lives.
        f.debug_struct("LocalCredentialProvider")
            .field("filename", &self.spec.filename)
            .finish_non_exhaustive()
    }
}

fn error(message: String) -> BoxError {
    message.into()
}

fn source_name(source: LaunchEnvironmentSource) -> &'static str {
    match source {
        LaunchEnvironmentSource::Process => "process",
        LaunchEnvironmentSource::ProjectEnv => "project-env",
        LaunchEnvironmentSource::UserEnv => "user-env",
    }
}

impl LocalCredentialProvider {
    /// Construct and initialize the provider (`new LocalCredentialProvider(ctx, config)` and its
    /// `Service.init`): the boot read, the watcher and the teardown effects. Failure here fails the
    /// plugin's activation, because a credentials document that exists but cannot be trusted must
    /// never be treated as "no credentials stored". The caller publishes the service.
    pub async fn start(
        ctx: &Context,
        config: &Config,
        seams: Seams,
    ) -> Result<Arc<Self>, BoxError> {
        let spec = resolve_spec(config)?;
        let provider = Arc::new_cyclic(|me| Self {
            ctx: ctx.clone(),
            spec,
            state: Mutex::new(State::default()),
            operations: tokio::sync::Mutex::new(()),
            closed: AtomicBool::new(false),
            writer: seams.writer.clone().unwrap_or_else(default_writer),
            me: me.clone(),
        });
        // Drain: refuse new operations, then settle the queued ones so disposal completes only
        // once storage is quiescent.
        let draining = provider.clone();
        ctx.effect("credentials-local drain", move || {
            Undo::future(move || async move {
                draining.closed.store(true, Ordering::SeqCst);
                drop(draining.operations.lock().await);
                Ok(())
            })
        })?;
        provider.load_initial().await?;
        if provider.spec.watch {
            provider.start_watcher(ctx, &seams).await?;
        }
        Ok(provider)
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    async fn start_watcher(&self, ctx: &Context, seams: &Seams) -> Result<(), BoxError> {
        let path =
            canonicalize_watch_path(&self.spec.filename)
                .await
                .map_err(|error| -> BoxError {
                    node_fs_error_message(
                        &error,
                        "stat",
                        Some(&self.spec.filename.to_string_lossy()),
                        None,
                    )
                    .into()
                })?;
        let weak = self.me.clone();
        let refresh = move || {
            if let Some(provider) = weak.upgrade() {
                // The initial load raced the watcher's own setup, and a change written between
                // that read and the watcher becoming active fires no event: a reconcile at
                // `ready` closes the gap, so `all` and `ready` queue the same refresh.
                if provider.is_closed() {
                    return;
                }
                tokio::spawn(async move { provider.queue_refresh().await });
            }
        };
        let weak = self.me.clone();
        let sink = WatchSink {
            all: Arc::new(refresh.clone()),
            ready: Arc::new(refresh),
            error: Arc::new(move |error| {
                // A watcher error after disposal is not actionable: the watched home is going
                // away with this provider, and logging it would be noise.
                if let Some(provider) = weak.upgrade()
                    && !provider.is_closed()
                {
                    provider.ctx.logger().warn(format_args!(
                        "credentials-local: watcher error on {}",
                        provider.spec.filename.display()
                    ));
                    provider.ctx.logger().warn(&error);
                }
            }),
        };
        let debounce = self.spec.debounce_ms;
        let request = WatchRequest {
            path,
            stability_threshold_ms: debounce as u64,
            poll_interval_ms: debounce.clamp(1.0, 10.0) as u64,
            sink: sink.clone(),
        };
        let factory = seams.watcher.clone().unwrap_or_else(watch::notify_factory);
        let watcher = match factory(request) {
            Ok(watcher) => watcher,
            Err(failure) => {
                // The document's directory may not exist yet; the provider still serves and
                // writes, and reads the document again at the next write.
                sink.error(failure);
                return Ok(());
            }
        };
        let quiescing = self.me.upgrade();
        let slot = Mutex::new(Some(watcher));
        ctx.effect("credentials-local watcher", move || {
            Undo::future(move || async move {
                // Quiesce: stop accepting events, close the watcher, then wait out any queued or
                // in-flight operation so nothing publishes after disposal.
                if let Some(provider) = &quiescing {
                    provider.closed.store(true, Ordering::SeqCst);
                }
                let watcher = lock(&slot).take();
                if let Some(watcher) = watcher {
                    watcher.close().await;
                }
                if let Some(provider) = &quiescing {
                    drop(provider.operations.lock().await);
                }
                Ok(())
            })
        })?;
        Ok(())
    }

    /// The inherited-environment value for a reference, or `None` when empty or unset.
    fn inherited(&self, reference: &str) -> Option<String> {
        launch_environment_of(&self.ctx)
            .get_from(reference, &[LaunchEnvironmentSource::Process])
            .filter(|entry| !entry.value.is_empty())
            .map(|entry| entry.value)
    }

    /// The `.env` fallback for a reference — below the managed store, never above it. The
    /// invoking project ranks over the user's home file: the more specific location wins.
    fn dotenv_fallback(&self, reference: &str) -> Option<(String, &'static str)> {
        launch_environment_of(&self.ctx)
            .get_from(
                reference,
                &[
                    LaunchEnvironmentSource::ProjectEnv,
                    LaunchEnvironmentSource::UserEnv,
                ],
            )
            .filter(|entry| !entry.value.is_empty())
            .map(|entry| (entry.value, source_name(entry.source)))
    }

    /// Queue one exclusive document operation behind every earlier one.
    async fn enqueue<T>(
        &self,
        operation: impl Future<Output = Result<T, BoxError>>,
    ) -> Result<T, BoxError> {
        let _turn = self.operations.lock().await;
        operation.await
    }

    /// Queue a reload; `refresh` contains its own failures.
    async fn queue_refresh(&self) {
        let _ = self
            .enqueue(async {
                self.refresh().await;
                Ok(())
            })
            .await;
    }

    async fn locked<T, F: Future<Output = Result<T, BoxError>>>(
        &self,
        operation: impl FnOnce() -> F,
    ) -> Result<T, BoxError> {
        ensure_directory(&self.spec.filename).await?;
        with_file_lock(
            &self.spec.filename,
            operation,
            FileLockOptions {
                wait_ms: Some(DOCUMENT_LOCK_WAIT_MS),
                ..FileLockOptions::default()
            },
        )
        .await
    }

    /// Persist the next document text and make it the cached one.
    async fn commit(&self, next_text: String) -> Result<(), BoxError> {
        // 0600: a document holding secrets is never world-readable.
        (self.writer)(self.spec.filename.clone(), next_text.clone())
            .await
            .map_err(|failure| -> BoxError {
                node_fs_error_message(
                    &failure,
                    "write",
                    Some(&self.spec.filename.to_string_lossy()),
                    None,
                )
                .into()
            })?;
        lock(&self.state).text = Some(next_text);
        Ok(())
    }

    /// The patched text must read back as the snapshot with exactly this change: a document the
    /// next boot would reject, or one that lost a neighbouring entry, is never written.
    fn verify(
        &self,
        next_text: &str,
        section: &str,
        key: &str,
        change: impl FnOnce(&mut IndexMap<String, String>, &mut IndexMap<String, CredentialRecord>),
        what: &str,
    ) -> Result<(), BoxError> {
        let (mut refs, mut records, previous) = {
            let state = lock(&self.state);
            (
                state.values.clone(),
                state.records.clone(),
                state.text.clone(),
            )
        };
        // Everything this provider does not manage must come through the rewrite untouched.
        if !edit::foreign_content_preserved(previous.as_deref(), next_text, section, key) {
            return Err(error(format!(
                "credentials-local: the rewrite after {what} would alter content this provider does not manage; nothing was written"
            )));
        }
        change(&mut refs, &mut records);
        let reread = parse_credentials_document(next_text, &self.spec.filename.to_string_lossy())
            .ok()
            .filter(|document| {
                document.refs == refs
                    && document.records.len() == records.len()
                    && records
                        .iter()
                        .all(|(key, record)| same_record(Some(record), document.records.get(key)))
            });
        match reread {
            Some(_) => Ok(()),
            None => Err(error(format!(
                "credentials-local: the document would not read back as intended after {what}; nothing was written"
            ))),
        }
    }

    /// Queue one line edit; disposal is checked on entry and again when the queued edit runs.
    async fn write(&self, reference: &str, value: Option<&str>) -> Result<(), BoxError> {
        let verb = if value.is_none() { "unset" } else { "set" };
        if self.is_closed() {
            return Err(error(format!(
                "credentials-local is disposed: cannot {verb} \"{reference}\""
            )));
        }
        self.enqueue(async {
            if self.is_closed() {
                return Err(error(format!(
                    "credentials-local was disposed before the queued \"{reference}\" {verb} ran"
                )));
            }
            self.locked(|| async {
                // Read-modify-write: fold in any on-disk state this process has not observed yet
                // — an external edit still inside the watcher debounce window, a change the
                // watcher missed, or another process's write — so the edit below can never
                // resurrect a stale document.
                self.reconcile_from_disk().await?;
                let (existing, text) = {
                    let state = lock(&self.state);
                    (state.values.get(reference).cloned(), state.text.clone())
                };
                if value.is_none() && existing.is_none() {
                    return Ok(());
                }
                let next_text = edit::render_ref(text.as_deref(), reference, value)?;
                self.verify(
                    &next_text,
                    "refs",
                    reference,
                    |refs, _| match value {
                        Some(value) => {
                            refs.insert(reference.to_owned(), value.to_owned());
                        }
                        None => {
                            refs.shift_remove(reference);
                        }
                    },
                    &format!("{verb} of \"{reference}\""),
                )?;
                self.commit(next_text).await?;
                {
                    let mut state = lock(&self.state);
                    match value {
                        Some(value) => {
                            state.values.insert(reference.to_owned(), value.to_owned());
                        }
                        None => {
                            state.values.shift_remove(reference);
                        }
                    }
                }
                // After the commit: a broken observer must never make the durable write look
                // failed.
                self.notify_updated(&self.ctx, &credential_ref(reference)?);
                Ok(())
            })
            .await
        })
        .await
    }

    /// Boot read: an absent file is an empty store; an invalid one fails the plugin's activation.
    /// The one exception is the recognized pre-release flat layout, which is upgraded in place
    /// first — a key stored by an earlier build must survive the layout change without a hand edit.
    async fn load_initial(&self) -> Result<(), BoxError> {
        assert_owner_only(&self.spec.filename).await?;
        let Some(mut text) = read_document(&self.spec.filename).await? else {
            return Ok(());
        };
        if render_flat_layout_migration(&text).is_some() {
            text = self.migrate_flat_document().await?;
        }
        let document = parse_credentials_document(&text, &self.spec.filename.to_string_lossy())?;
        self.warn_ignored_keys(&document);
        let mut state = lock(&self.state);
        state.values = document.refs;
        state.records = document.records;
        state.text = Some(text);
        Ok(())
    }

    /// One-shot upgrade of the recognized pre-release flat layout, before the watcher exists. The
    /// rewrite runs under the document's writer lock and re-reads first — a concurrent boot may
    /// have migrated already — and whatever the re-read finds that is not the flat layout is
    /// returned untouched for the ordinary parse.
    async fn migrate_flat_document(&self) -> Result<String, BoxError> {
        self.locked(|| async {
            let current = read_document(&self.spec.filename)
                .await?
                .ok_or_else(|| error("credentials-local: the document vanished during migration".into()))?;
            let Some(migrated) = render_flat_layout_migration(&current) else {
                return Ok(current);
            };
            (self.writer)(self.spec.filename.clone(), migrated.clone())
                .await
                .map_err(|failure| -> BoxError {
                    node_fs_error_message(
                        &failure,
                        "write",
                        Some(&self.spec.filename.to_string_lossy()),
                        None,
                    )
                    .into()
                })?;
            self.ctx.logger().info(format_args!(
                "credentials-local: migrated {} to the version {DOCUMENT_VERSION} layout; values are unchanged",
                self.spec.filename.display()
            ));
            Ok(migrated)
        })
        .await
    }

    /// Re-read the document after a watcher event. Unchanged content (including this provider's
    /// own writes) is a no-op; an unreadable document keeps the last good snapshot and warns — a
    /// live hot-reload must never take the process down.
    async fn refresh(&self) {
        if self.is_closed() {
            return;
        }
        if let Err(failure) = self.reconcile_from_disk().await {
            self.ctx.logger().warn(format_args!(
                "credentials-local: reload failed at {}; keeping the last good document",
                self.spec.filename.display()
            ));
            self.ctx.logger().warn(&failure);
        }
    }

    /// Compare the on-disk text against the cache and publish any difference into the seam.
    /// Absence publishes the empty store; an unreadable or invalid document fails, so each caller
    /// picks its policy — a reload warns and keeps the last good snapshot, a write fails loud
    /// rather than overwriting a document it could not understand.
    async fn reconcile_from_disk(&self) -> Result<(), BoxError> {
        // Re-checked on every reload and before every write: an external editor or a restored
        // backup can loosen the mode after boot.
        assert_owner_only(&self.spec.filename).await?;
        let text = read_document(&self.spec.filename).await?;
        if text == lock(&self.state).text || self.is_closed() {
            return Ok(());
        }
        let next = match &text {
            None => CredentialsDocument::default(),
            Some(text) => parse_credentials_document(text, &self.spec.filename.to_string_lossy())?,
        };
        self.warn_ignored_keys(&next);
        let (changed_refs, changed_records) = {
            let mut state = lock(&self.state);
            let changed_refs = changed_refs(&state.values, &next.refs);
            let changed_records = changed_records(&state.records, &next.records);
            state.text = text;
            state.values = next.refs;
            state.records = next.records;
            (changed_refs, changed_records)
        };
        for reference in changed_refs {
            self.notify_updated(&self.ctx, &credential_ref(&reference)?);
        }
        for key in changed_records {
            self.notify_record_updated(&self.ctx, &parse_credential_key(&key)?);
        }
        Ok(())
    }

    /// Report top-level keys that resolve nothing; only key names are printed, never values.
    fn warn_ignored_keys(&self, document: &CredentialsDocument) {
        if document.ignored_keys.is_empty() {
            return;
        }
        let names = document
            .ignored_keys
            .iter()
            .map(|key| js_json_stringify(&serde_json::Value::String(key.clone())))
            .collect::<Vec<_>>()
            .join(", ");
        self.ctx.logger().warn(format_args!(
            "credentials-local: ignoring unknown top-level keys {names} in {}; only `refs` and `records` entries resolve",
            self.spec.filename.display()
        ));
    }
}

/// Entries whose stored value changed; the parser has already proven every key addressable.
fn changed_refs(prev: &IndexMap<String, String>, next: &IndexMap<String, String>) -> Vec<String> {
    let mut keys: Vec<&String> = prev.keys().collect();
    keys.extend(next.keys().filter(|key| !prev.contains_key(*key)));
    keys.into_iter()
        .filter(|key| prev.get(*key) != next.get(*key))
        .cloned()
        .collect()
}

/// Records whose stored value changed.
fn changed_records(
    prev: &IndexMap<String, CredentialRecord>,
    next: &IndexMap<String, CredentialRecord>,
) -> Vec<String> {
    let mut keys: Vec<&String> = prev.keys().collect();
    keys.extend(next.keys().filter(|key| !prev.contains_key(*key)));
    keys.into_iter()
        .filter(|key| !same_record(prev.get(*key), next.get(*key)))
        .cloned()
        .collect()
}

impl CredentialProvider for LocalCredentialProvider {
    fn resolve<'a>(
        &'a self,
        reference: &'a CredentialRef,
    ) -> CredentialFuture<'a, Option<ResolvedCredential>> {
        Box::pin(async move {
            let name = reference.as_str();
            if let Some(stored) = lock(&self.state).values.get(name).cloned() {
                return Ok(Some(ResolvedCredential {
                    value: stored,
                    source: "file".into(),
                }));
            }
            if let Some(value) = self.inherited(name) {
                return Ok(Some(ResolvedCredential {
                    value,
                    source: "env".into(),
                }));
            }
            Ok(self
                .dotenv_fallback(name)
                .map(|(value, source)| ResolvedCredential {
                    value,
                    source: source.into(),
                }))
        })
    }

    fn describe<'a>(
        &'a self,
        reference: &'a CredentialRef,
    ) -> CredentialFuture<'a, CredentialInfo> {
        Box::pin(async move {
            let name = reference.as_str();
            if let Some(stored) = lock(&self.state).values.get(name) {
                return Ok(CredentialInfo {
                    configured: true,
                    source: Some("file".into()),
                    writable: true,
                    masked: Some(mask_credential(stored)),
                });
            }
            // Every reference is writable: a set writes the managed file, which outranks every
            // fallback, and an unset exposes the fallback again.
            if self.inherited(name).is_some() {
                return Ok(CredentialInfo {
                    configured: true,
                    source: Some("env".into()),
                    writable: true,
                    masked: None,
                });
            }
            Ok(match self.dotenv_fallback(name) {
                Some((_, source)) => CredentialInfo {
                    configured: true,
                    source: Some(source.into()),
                    writable: true,
                    masked: None,
                },
                None => CredentialInfo {
                    configured: false,
                    source: None,
                    writable: true,
                    masked: None,
                },
            })
        })
    }

    fn set<'a>(&'a self, reference: &'a CredentialRef, value: &'a str) -> CredentialFuture<'a, ()> {
        Box::pin(async move {
            if value.is_empty() {
                return Err(error(format!(
                    "credentials-local: an empty value cannot be stored for \"{reference}\"; use unset"
                )));
            }
            self.write(reference.as_str(), Some(value)).await
        })
    }

    fn unset<'a>(&'a self, reference: &'a CredentialRef) -> CredentialFuture<'a, ()> {
        Box::pin(async move { self.write(reference.as_str(), None).await })
    }

    fn read_record<'a>(
        &'a self,
        key: &'a CredentialKey,
    ) -> CredentialFuture<'a, Option<CredentialRecord>> {
        Box::pin(async move { Ok(lock(&self.state).records.get(key.as_str()).cloned()) })
    }

    fn describe_record<'a>(
        &'a self,
        key: &'a CredentialKey,
    ) -> CredentialFuture<'a, CredentialRecordInfo> {
        Box::pin(async move {
            // Presence is the whole fact here: no layer ranks above this document for a record,
            // so nothing can shadow one, and an api-key record carrying neither a key nor
            // environment values is a deliberate statement rather than a blank.
            Ok(match lock(&self.state).records.get(key.as_str()) {
                None => CredentialRecordInfo {
                    configured: false,
                    kind: None,
                    writable: true,
                },
                Some(record) => CredentialRecordInfo {
                    configured: true,
                    kind: Some(record.kind()),
                    writable: true,
                },
            })
        })
    }

    fn list_records(&self) -> CredentialFuture<'_, Vec<CredentialRecordEntry>> {
        Box::pin(async move {
            lock(&self.state)
                .records
                .iter()
                .map(|(key, record)| {
                    Ok(CredentialRecordEntry {
                        // The parser has already proven every stored key addressable.
                        key: parse_credential_key(key)?,
                        kind: record.kind(),
                    })
                })
                .collect()
        })
    }

    fn modify_record<'a>(
        &'a self,
        key: &'a CredentialKey,
        mutate: CredentialMutator,
    ) -> CredentialFuture<'a, Option<CredentialRecord>> {
        Box::pin(async move {
            if self.is_closed() {
                return Err(error(format!(
                    "credentials-local is disposed: cannot modify \"{key}\""
                )));
            }
            self.enqueue(async {
                if self.is_closed() {
                    return Err(error(format!(
                        "credentials-local was disposed before the queued \"{key}\" modify ran"
                    )));
                }
                self.locked(|| async {
                    // Read-modify-write: `mutate` must decide against the record as it stands
                    // now, not as this process last saw it — another process may have rotated it.
                    self.reconcile_from_disk().await?;
                    let (current, text) = {
                        let state = lock(&self.state);
                        (state.records.get(key.as_str()).cloned(), state.text.clone())
                    };
                    let Some(next) = mutate(current.clone()).await? else {
                        return Ok(current);
                    };
                    // Admitted before it is rendered: what the read path would refuse is refused
                    // here first, so a caller can never persist a document the next boot rejects.
                    if let CredentialRecord::ApiKey(record) = &next {
                        assert_storable_api_key(key.as_str(), record)?;
                    }
                    let next_text =
                        edit::render_record(text.as_deref(), key.as_str(), Some(&next))?;
                    self.verify(
                        &next_text,
                        "records",
                        key.as_str(),
                        |_, records| {
                            records.insert(key.as_str().to_owned(), next.clone());
                        },
                        &format!("the write of record \"{key}\""),
                    )?;
                    self.commit(next_text).await?;
                    lock(&self.state)
                        .records
                        .insert(key.as_str().to_owned(), next.clone());
                    // After the commit, on the same terms as a reference write.
                    self.notify_record_updated(&self.ctx, key);
                    Ok(Some(next))
                })
                .await
            })
            .await
        })
    }

    fn delete_record<'a>(&'a self, key: &'a CredentialKey) -> CredentialFuture<'a, ()> {
        Box::pin(async move {
            if self.is_closed() {
                return Err(error(format!(
                    "credentials-local is disposed: cannot delete \"{key}\""
                )));
            }
            self.enqueue(async {
                if self.is_closed() {
                    return Err(error(format!(
                        "credentials-local was disposed before the queued \"{key}\" delete ran"
                    )));
                }
                self.locked(|| async {
                    self.reconcile_from_disk().await?;
                    let text = {
                        let state = lock(&self.state);
                        if !state.records.contains_key(key.as_str()) {
                            return Ok(());
                        }
                        state.text.clone()
                    };
                    let next_text = edit::render_record(text.as_deref(), key.as_str(), None)?;
                    self.verify(
                        &next_text,
                        "records",
                        key.as_str(),
                        |_, records| {
                            records.shift_remove(key.as_str());
                        },
                        &format!("the delete of record \"{key}\""),
                    )?;
                    self.commit(next_text).await?;
                    lock(&self.state).records.shift_remove(key.as_str());
                    self.notify_record_updated(&self.ctx, key);
                    Ok(())
                })
                .await
            })
            .await
        })
    }
}

impl Plugin for LocalCredentialProvider {
    const NAME: &'static str = "LocalCredentialProvider";
    type Config = Config;
    type Deps = ();

    async fn apply(ctx: Context, _deps: (), config: Cfg<Config>) -> Result<(), BoxError> {
        let provider = Self::start(&ctx, &config.get(), Seams::default()).await?;
        ctx.provide::<Credentials>(provider).await?;
        Ok(())
    }
}
