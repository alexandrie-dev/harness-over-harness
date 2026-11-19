//! Stdio composition of the window profile: one reader of the process stdin, shared by the
//! JSON-RPC server (the bytes) and `hoh-sdk-app` (only the end of the stream).

use futures_util::future::BoxFuture;
use hoh_commander::Output;
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, NoConfig, Plugin, Service};
use hoh_loader::ProfileContextKey;
use hoh_loader::cmdline::{AppStdin, Listener, Subscription};
use hoh_model_host::{ModelHostKey, PersistedSettings};
use hoh_protocol::{ModelSettings, SettingsRulesSetParams, SettingsSetParams};
use hoh_sdk_app::{SdkAppRuntimeHooks, SdkAppRuntimeKey};
use hoh_sdk_jsonrpc_server::{
    HohPrivilegedApprover, JsonRpcRuntime, JsonRpcRuntimeHooks, PrivilegedApprover,
};
use hoh_user_approval::Approval;
use serde_json::Value;
use std::{
    path::Path,
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard},
    task::{Context as TaskContext, Poll},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWriteExt, BufReader, ReadBuf};

/// The only profile that takes its stdio through this seam.
pub const WINDOW_PROFILE: &str = "window";
const PIPE_CAPACITY: usize = 64 * 1024;
const STDIN_CHUNK: usize = 8 * 1024;

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Default)]
struct EndState {
    ended: bool,
    next: u64,
    listeners: Vec<(u64, Listener)>,
}

/// End of the process stdin, observed without consuming a byte.
#[derive(Clone, Default)]
struct StdinEnd(Arc<Mutex<EndState>>);

impl StdinEnd {
    fn fire(&self) {
        let listeners = {
            let mut state = lock(&self.0);
            state.ended = true;
            std::mem::take(&mut state.listeners)
        };
        for (_, listener) in listeners {
            listener();
        }
    }
}

impl AppStdin for StdinEnd {
    fn readable_ended(&self) -> bool {
        lock(&self.0).ended
    }

    fn once_end(&self, listener: Listener) -> Subscription {
        let token = {
            let mut state = lock(&self.0);
            if !state.ended {
                state.next += 1;
                let token = state.next;
                state.listeners.push((token, listener));
                Some(token)
            } else {
                drop(state);
                listener();
                None
            }
        };
        let shared = self.0.clone();
        Subscription::new(move || {
            if let Some(token) = token {
                lock(&shared).listeners.retain(|(id, _)| *id != token);
            }
        })
    }
}

/// The server's end of the stdin pipe. The end of input is announced only once the reader has
/// taken every byte: that is when Node emits `end`, and what keeps a piped `initialize` +
/// `session/prompt` from being cut off by the exit that follows.
struct EndAnnouncing<R> {
    inner: R,
    end: StdinEnd,
}

impl<R: AsyncRead + Unpin> AsyncRead for EndAnnouncing<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(cx, buf);
        if matches!(polled, Poll::Ready(Ok(()))) && buf.filled().len() == before {
            self.end.fire();
        }
        polled
    }
}

/// Copy the process stdin into the pipe from a plain thread. A blocking read on tokio's pool would
/// keep the runtime from shutting down while a client holds its end open; a thread that is still
/// reading simply dies with the process.
fn forward_stdin(mut writer: tokio::io::DuplexStream) -> std::io::Result<()> {
    let (chunks, mut received) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
    std::thread::Builder::new()
        .name("hoh-stdin".into())
        .spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut buffer = vec![0_u8; STDIN_CHUNK];
            loop {
                match std::io::Read::read(&mut stdin, &mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        if chunks.blocking_send(buffer[..read].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        })?;
    tokio::spawn(async move {
        while let Some(chunk) = received.recv().await {
            // A closed pipe means the server is gone; the client is no longer served either way.
            if writer.write_all(&chunk).await.is_err() {
                return;
            }
        }
        let _ = writer.shutdown().await;
    });
    Ok(())
}

/// Hand the process stdio to the window profile before its plugins mount.
///
/// The server gets the bytes through an in-process pipe and the process stdout; `hoh-sdk-app`
/// gets the end-of-input signal; after a successful `shutdown` the server calls `exiting` before it
/// disposes the kernel root and `request_exit` once the disposal is done.
pub async fn provide_stdio(
    ctx: &Context,
    exiting: impl Fn() + Send + Sync + 'static,
    request_exit: impl Fn(i32) + Send + Sync + 'static,
) -> Result<(), BoxError> {
    let end = StdinEnd::default();
    let (writer, reader) = tokio::io::duplex(PIPE_CAPACITY);
    forward_stdin(writer)?;
    let reader = EndAnnouncing {
        inner: reader,
        end: end.clone(),
    };
    ctx.provide::<JsonRpcRuntime>(Arc::new(
        JsonRpcRuntimeHooks::new(
            Box::new(BufReader::new(reader)),
            Box::new(tokio::io::stdout()),
            request_exit,
        )
        .with_exiting(exiting),
    ))
    .await?;
    ctx.provide::<SdkAppRuntimeKey>(Arc::new(SdkAppRuntimeHooks::new(
        Arc::new(end),
        Output::default(),
    )))
    .await?;
    Ok(())
}

/// Where the user's model address and name outlive the engine process.
pub const MODEL_SETTINGS_FILE: &str = "model-settings.json";
/// The first-run rules (internet mode and the allowed folder), next to it.
pub const WINDOW_RULES_FILE: &str = "window-rules.json";
const SAVE_RETRY: Duration = Duration::from_secs(1);

#[derive(Inject)]
pub struct WindowModelSettingsDeps {
    host: Service<ModelHostKey>,
    profile: Service<ProfileContextKey>,
}

/// Keeps `hoh/settings/set` model changes across restarts. `hoh-model-host` holds the address and
/// name in memory and starts from its config; this row restores the last saved pair over that
/// config and saves each later change next to the profile (never the key: that is in credentials).
pub struct WindowModelSettings;

impl Plugin for WindowModelSettings {
    const NAME: &'static str = "window-model-settings";
    type Config = NoConfig;
    type Deps = WindowModelSettingsDeps;

    async fn apply(
        ctx: Context,
        deps: Self::Deps,
        _config: Cfg<Self::Config>,
    ) -> Result<(), BoxError> {
        let path = deps.profile.dir.join(MODEL_SETTINGS_FILE);
        let rules_path = deps.profile.dir.join(WINDOW_RULES_FILE);
        let host = deps.host.clone();
        let logger = ctx.logger();
        if let Some(model) = load_saved::<ModelSettings>(&path, &logger).await {
            let restored = if address_carries_credentials(&model.base_url) {
                Err("the saved model address carries credentials".into())
            } else {
                host.set_settings(SettingsSetParams {
                    model: Some(model),
                    security: None,
                    view: None,
                })
                .map(drop)
            };
            if let Err(error) = restored {
                logger.warn(format!(
                    "saved model settings {} were not applied: {error}",
                    path.display()
                ));
            }
        }
        if let Some(rules) = load_saved::<SettingsRulesSetParams>(&rules_path, &logger).await
            && let Err(error) = host.set_rules(rules)
        {
            logger.warn(format!(
                "saved rules {} were not applied: {error}",
                rules_path.display()
            ));
        }
        let mut changes = host.subscribe();
        let mut saved = changes.borrow_and_update().clone();
        ctx.spawn("window-model-settings", move |token| async move {
            let mut failing = false;
            loop {
                // A failed write is retried; otherwise the next change (or the end) is what wakes us.
                let last = tokio::select! {
                    () = token.cancelled() => true,
                    changed = changes.changed() => changed.is_err(),
                    () = tokio::time::sleep(SAVE_RETRY), if failing => false,
                };
                let current = changes.borrow_and_update().clone();
                failing = !persist(&current, &mut saved, &path, &rules_path, &logger).await;
                if last {
                    return Ok(());
                }
            }
        })?;
        Ok(())
    }
}

/// Write what differs from `saved`; `false` when a write failed (and will be tried again).
async fn persist(
    current: &PersistedSettings,
    saved: &mut PersistedSettings,
    model_path: &Path,
    rules_path: &Path,
    logger: &hoh_kernel::Logger,
) -> bool {
    let mut ok = true;
    if current.model != saved.model {
        match save(model_path, &current.model).await {
            Ok(()) => saved.model = current.model.clone(),
            Err(error) => {
                ok = false;
                logger.warn(format!(
                    "model settings could not be saved to {}: {error}",
                    model_path.display()
                ));
            }
        }
    }
    if current.rules != saved.rules
        && let Some(rules) = &current.rules
    {
        match save(rules_path, rules).await {
            Ok(()) => saved.rules = current.rules.clone(),
            Err(error) => {
                ok = false;
                logger.warn(format!(
                    "rules could not be saved to {}: {error}",
                    rules_path.display()
                ));
            }
        }
    }
    ok
}

/// The saved value of one file: `None` for a missing file, and for one that cannot be used (logged).
async fn load_saved<T: serde::de::DeserializeOwned>(
    path: &Path,
    logger: &hoh_kernel::Logger,
) -> Option<T> {
    match tokio::fs::read(path).await {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(value) => Some(value),
            Err(error) => {
                logger.warn(format!(
                    "saved settings {} are unusable: {error}",
                    path.display()
                ));
                None
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            logger.warn(format!(
                "saved settings {} could not be read: {error}",
                path.display()
            ));
            None
        }
    }
}

async fn save(path: &Path, value: &impl serde::Serialize) -> std::io::Result<()> {
    let text = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    hoh_atomic_write::write_file_atomic(
        path,
        text,
        hoh_atomic_write::WriteFileAtomicOptions {
            mode: 0o600,
            dir_mode: None,
        },
    )
    .await?;
    // `write_file_atomic` renames but does not sync: make the new content survive a power cut.
    tokio::fs::File::open(path).await?.sync_all().await?;
    if let Some(dir) = path.parent() {
        tokio::fs::File::open(dir).await?.sync_all().await?;
    }
    Ok(())
}

/// The model address is shown to the user, saved and returned by `hoh/settings/get`: it must not
/// carry a secret (`https://user:KEY@host/v1`, `?api_key=...`). The key has its own field.
pub fn address_carries_credentials(address: &str) -> bool {
    let Ok(url) = url::Url::parse(address) else {
        return false;
    };
    if !url.username().is_empty() || url.password().is_some() {
        return true;
    }
    url.query_pairs().any(|(name, _)| {
        let name = name.to_ascii_lowercase();
        [
            "key",
            "token",
            "secret",
            "password",
            "passwd",
            "auth",
            "sig",
            "credential",
        ]
        .iter()
        .any(|word| name.contains(word))
    })
}

fn model_address(method: &str, params: &Value) -> Option<String> {
    let address = match method {
        "hoh/settings/set" => params.pointer("/model/baseUrl"),
        "hoh/model/key/set" | "hoh/model/probe" => params.get("baseUrl"),
        _ => None,
    };
    address.and_then(Value::as_str).map(str::to_owned)
}

#[derive(Inject)]
pub struct WindowPrivilegedApproverDeps {
    approval: Service<Approval>,
}

/// Mounts the server's privileged-call gate on the approval service: the server asks the
/// `HohPrivilegedApprover` service before running a privileged `hoh/*` call and nothing else
/// provides it. (The server could call `Approval::approve_privileged` itself, as it already depends
/// on `hoh-user-approval`; until it does, this row is what makes those calls possible.) A model
/// address that carries a credential is refused before the user is asked about it.
pub struct WindowPrivilegedApprover;

struct Bridge(Service<Approval>);

impl PrivilegedApprover for Bridge {
    fn approve(&self, method: &str, params: &Value) -> BoxFuture<'static, Result<(), BoxError>> {
        if model_address(method, params)
            .is_some_and(|address| address_carries_credentials(&address))
        {
            return Box::pin(async {
                Err::<(), BoxError>(
                    "the model address must not contain a user name, password or key".into(),
                )
            });
        }
        self.0.approve_privileged(method, params)
    }
}

impl Plugin for WindowPrivilegedApprover {
    const NAME: &'static str = "window-privileged-approver";
    type Config = NoConfig;
    type Deps = WindowPrivilegedApproverDeps;

    async fn apply(
        ctx: Context,
        deps: Self::Deps,
        _config: Cfg<Self::Config>,
    ) -> Result<(), BoxError> {
        ctx.provide::<HohPrivilegedApprover>(Arc::new(Bridge(deps.approval)))
            .await?;
        Ok(())
    }
}
