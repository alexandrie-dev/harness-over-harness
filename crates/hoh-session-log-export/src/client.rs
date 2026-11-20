//! Browser-controller semantics behind transport and clipboard adapters.

use crate::{SESSION_LOG_EXPORT_ROUTE, SESSION_MARKDOWN_EXPORT_ROUTE, session_log_zip_filename};
use futures_util::{
    FutureExt,
    future::{BoxFuture, Shared, join_all, join3},
};
use hoh_error::{BoxError, SharedError};
use hoh_timeout::{AbortController, AbortSignal};
use std::{
    collections::HashMap,
    io,
    sync::{Arc, Mutex, MutexGuard},
};
use tokio::sync::watch;

pub type SettledOperation = Shared<BoxFuture<'static, ()>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionLogDownloadStatus {
    Downloading,
    Success,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionLogDownloadEntry {
    pub order: u64,
    pub open: bool,
    pub status: SessionLogDownloadStatus,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionLogDownloadState {
    pub by_session: HashMap<String, SessionLogDownloadEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientResponse {
    pub status: u16,
    pub body: String,
}

impl ClientResponse {
    pub fn ok(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            body: body.into(),
        }
    }
    pub fn is_ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

pub trait SessionLogDownloadAdapter: Send + Sync + 'static {
    fn preflight(
        &self,
        route: String,
        signal: AbortSignal,
    ) -> BoxFuture<'static, Result<ClientResponse, BoxError>>;
    fn save(&self, route: &str, filename: &str) -> Result<(), BoxError>;
}

struct DownloadActive {
    order: u64,
    abort: Arc<AbortController>,
    done: SettledOperation,
}

struct DownloadInner {
    adapter: Arc<dyn SessionLogDownloadAdapter>,
    state: Mutex<DownloadMutable>,
    notifications: watch::Sender<SessionLogDownloadState>,
}

#[derive(Default)]
struct DownloadMutable {
    snapshot: SessionLogDownloadState,
    active: HashMap<String, DownloadActive>,
    disposed: bool,
    next_order: u64,
}

#[derive(Clone)]
pub struct SessionLogDownloadController {
    inner: Arc<DownloadInner>,
}

impl SessionLogDownloadController {
    pub fn new(adapter: Arc<dyn SessionLogDownloadAdapter>) -> Self {
        let (notifications, _) = watch::channel(SessionLogDownloadState::default());
        Self {
            inner: Arc::new(DownloadInner {
                adapter,
                state: Mutex::new(DownloadMutable::default()),
                notifications,
            }),
        }
    }

    pub fn state(&self) -> SessionLogDownloadState {
        lock(&self.inner.state).snapshot.clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<SessionLogDownloadState> {
        self.inner.notifications.subscribe()
    }

    pub fn download(&self, session_id: impl Into<String>) -> SettledOperation {
        let session_id = session_id.into();
        let mut state = lock(&self.inner.state);
        if let Some(active) = state.active.get(&session_id) {
            return active.done.clone();
        }
        if state.disposed {
            return settled();
        }
        let order = state.next_order;
        state.next_order += 1;
        state.snapshot.by_session.insert(
            session_id.clone(),
            SessionLogDownloadEntry {
                order,
                open: true,
                status: SessionLogDownloadStatus::Downloading,
                error: None,
            },
        );
        self.inner
            .notifications
            .send_replace(state.snapshot.clone());
        let abort = Arc::new(AbortController::new());
        let signal = abort.signal();
        let inner = self.inner.clone();
        let operation_id = session_id.clone();
        let done = async move {
            run_download(&inner, &operation_id, order, signal).await;
            let mut state = lock(&inner.state);
            if state
                .active
                .get(&operation_id)
                .is_some_and(|active| active.order == order)
            {
                state.active.remove(&operation_id);
            }
        }
        .boxed()
        .shared();
        state.active.insert(
            session_id,
            DownloadActive {
                order,
                abort,
                done: done.clone(),
            },
        );
        drop(state);
        tokio::spawn(done.clone());
        done
    }

    pub fn dismiss(&self, session_id: &str) {
        let mut state = lock(&self.inner.state);
        if let Some(entry) = state.snapshot.by_session.get_mut(session_id)
            && entry.open
        {
            entry.open = false;
            self.inner
                .notifications
                .send_replace(state.snapshot.clone());
        }
    }

    pub async fn dispose(&self) {
        let active = {
            let mut state = lock(&self.inner.state);
            state.disposed = true;
            state
                .active
                .values()
                .map(|active| (active.abort.clone(), active.done.clone()))
                .collect::<Vec<_>>()
        };
        for (abort, _) in &active {
            abort.abort();
        }
        join_all(active.into_iter().map(|(_, done)| done)).await;
    }
}

async fn run_download(
    inner: &Arc<DownloadInner>,
    session_id: &str,
    order: u64,
    signal: AbortSignal,
) {
    let route = download_route(session_id);
    let result = async {
        let response = inner
            .adapter
            .preflight(route.clone(), signal.clone())
            .await?;
        if !response.is_ok() {
            let suffix = if response.body.is_empty() {
                String::new()
            } else {
                format!(" {}", response.body)
            };
            return Err(boxed_message(format!(
                "Export failed: HTTP {}{suffix}",
                response.status
            )));
        }
        if signal.aborted() {
            return Ok(());
        }
        inner
            .adapter
            .save(&route, &session_log_zip_filename(session_id))
    }
    .await;
    if signal.aborted() {
        return;
    }
    let mut state = lock(&inner.state);
    let open = state
        .snapshot
        .by_session
        .get(session_id)
        .is_none_or(|entry| entry.open);
    state.snapshot.by_session.insert(
        session_id.to_owned(),
        match result {
            Ok(()) => SessionLogDownloadEntry {
                order,
                open,
                status: SessionLogDownloadStatus::Success,
                error: None,
            },
            Err(error) => SessionLogDownloadEntry {
                order,
                open,
                status: SessionLogDownloadStatus::Error,
                error: Some(error.to_string()),
            },
        },
    );
    inner.notifications.send_replace(state.snapshot.clone());
}

pub fn download_route(session_id: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("sessionId", session_id)
        .append_pair("includeDescendants", "true")
        .finish();
    format!("{SESSION_LOG_EXPORT_ROUTE}?{query}")
}

pub fn markdown_route(session_id: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("sessionId", session_id)
        .finish();
    format!("{SESSION_MARKDOWN_EXPORT_ROUTE}?{query}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionMarkdownCopyNoticeKind {
    Success,
    PreparationFailed,
    ClipboardFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionMarkdownCopyNotice {
    pub kind: SessionMarkdownCopyNoticeKind,
    pub seq: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionMarkdownCopyState {
    pub copying: bool,
    pub notice: Option<SessionMarkdownCopyNotice>,
}

pub trait SessionMarkdownCopyAdapter: Send + Sync + 'static {
    fn fetch_markdown(
        &self,
        route: String,
        signal: AbortSignal,
    ) -> BoxFuture<'static, Result<ClientResponse, BoxError>>;
    fn supports_deferred_write(&self) -> bool;
    /// Called synchronously by [`SessionMarkdownCopyController::copy`].
    fn write_deferred(
        &self,
        data: BoxFuture<'static, Result<Arc<String>, SharedError>>,
    ) -> Result<BoxFuture<'static, Result<(), BoxError>>, BoxError>;
    fn write_text(&self, text: String) -> BoxFuture<'static, Result<bool, BoxError>>;
}

struct CopyPending {
    id: u64,
    abort: Arc<AbortController>,
    done: SettledOperation,
}
struct CopyInner {
    adapter: Arc<dyn SessionMarkdownCopyAdapter>,
    state: Mutex<CopyMutable>,
    notifications: watch::Sender<SessionMarkdownCopyState>,
}

#[derive(Default)]
struct CopyMutable {
    snapshot: SessionMarkdownCopyState,
    pending: Option<CopyPending>,
    disposed: bool,
    next_operation: u64,
    notice_seq: u64,
}

#[derive(Clone)]
pub struct SessionMarkdownCopyController {
    inner: Arc<CopyInner>,
}

impl SessionMarkdownCopyController {
    pub fn new(adapter: Arc<dyn SessionMarkdownCopyAdapter>) -> Self {
        let (notifications, _) = watch::channel(SessionMarkdownCopyState::default());
        Self {
            inner: Arc::new(CopyInner {
                adapter,
                state: Mutex::new(CopyMutable::default()),
                notifications,
            }),
        }
    }

    pub fn state(&self) -> SessionMarkdownCopyState {
        lock(&self.inner.state).snapshot.clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<SessionMarkdownCopyState> {
        self.inner.notifications.subscribe()
    }

    pub fn copy(&self, session_id: impl Into<String>) -> SettledOperation {
        let session_id = session_id.into();
        let mut state = lock(&self.inner.state);
        if state.disposed {
            return settled();
        }
        if let Some(pending) = &state.pending {
            return pending.done.clone();
        }
        state.snapshot.copying = true;
        self.inner
            .notifications
            .send_replace(state.snapshot.clone());
        let id = state.next_operation;
        state.next_operation += 1;
        let abort = Arc::new(AbortController::new());
        let signal = abort.signal();
        let adapter = self.inner.adapter.clone();
        let preparation = async move {
            let response = adapter
                .fetch_markdown(markdown_route(&session_id), signal)
                .await
                .map_err(SharedError::new)?;
            if !response.is_ok() {
                return Err(SharedError::new(boxed_message(format!(
                    "Session Markdown request failed: HTTP {}",
                    response.status,
                ))));
            }
            Ok(Arc::new(response.body))
        }
        .boxed()
        .shared();

        let inner = self.inner.clone();
        let done = if inner.adapter.supports_deferred_write() {
            let data_inner = inner.clone();
            let data_preparation = preparation.clone();
            let data = async move {
                let markdown = data_preparation.await?;
                if lock(&data_inner.state).disposed {
                    return Err(SharedError::new(boxed_message(
                        "Session Markdown copy cancelled".into(),
                    )));
                }
                Ok(markdown)
            }
            .boxed()
            .shared();
            let write = inner
                .adapter
                .write_deferred(data.clone().boxed())
                .unwrap_or_else(|error| async move { Err(error) }.boxed());
            async move {
                let (prepared, data, written) = join3(preparation, data, write).await;
                if !lock(&inner.state).disposed {
                    if prepared.is_err() || data.is_err() {
                        publish_copy_notice(
                            &inner,
                            SessionMarkdownCopyNoticeKind::PreparationFailed,
                        );
                    } else if written.is_err() {
                        publish_copy_notice(&inner, SessionMarkdownCopyNoticeKind::ClipboardFailed);
                    } else {
                        publish_copy_notice(&inner, SessionMarkdownCopyNoticeKind::Success);
                    }
                }
                finish_copy(&inner, id);
            }
            .boxed()
            .shared()
        } else {
            async move {
                match preparation.await {
                    Err(_) => {
                        if !lock(&inner.state).disposed {
                            publish_copy_notice(
                                &inner,
                                SessionMarkdownCopyNoticeKind::PreparationFailed,
                            );
                        }
                    }
                    Ok(markdown) if !lock(&inner.state).disposed => {
                        let accepted = inner
                            .adapter
                            .write_text((*markdown).clone())
                            .await
                            .unwrap_or(false);
                        if !lock(&inner.state).disposed {
                            publish_copy_notice(
                                &inner,
                                if accepted {
                                    SessionMarkdownCopyNoticeKind::Success
                                } else {
                                    SessionMarkdownCopyNoticeKind::ClipboardFailed
                                },
                            );
                        }
                    }
                    Ok(_) => {}
                }
                finish_copy(&inner, id);
            }
            .boxed()
            .shared()
        };
        state.pending = Some(CopyPending {
            id,
            abort,
            done: done.clone(),
        });
        drop(state);
        tokio::spawn(done.clone());
        done
    }

    pub fn dismiss(&self) {
        let mut state = lock(&self.inner.state);
        if state.snapshot.notice.take().is_some() {
            self.inner
                .notifications
                .send_replace(state.snapshot.clone());
        }
    }

    pub async fn dispose(&self) {
        let pending = {
            let mut state = lock(&self.inner.state);
            state.disposed = true;
            state
                .pending
                .as_ref()
                .map(|pending| (pending.abort.clone(), pending.done.clone()))
        };
        if let Some((abort, done)) = pending {
            abort.abort();
            done.await;
        }
        let mut state = lock(&self.inner.state);
        state.snapshot = SessionMarkdownCopyState::default();
        self.inner
            .notifications
            .send_replace(state.snapshot.clone());
    }
}

fn publish_copy_notice(inner: &Arc<CopyInner>, kind: SessionMarkdownCopyNoticeKind) {
    let mut state = lock(&inner.state);
    state.notice_seq += 1;
    state.snapshot.notice = Some(SessionMarkdownCopyNotice {
        kind,
        seq: state.notice_seq,
    });
    inner.notifications.send_replace(state.snapshot.clone());
}

fn finish_copy(inner: &Arc<CopyInner>, id: u64) {
    let mut state = lock(&inner.state);
    state.snapshot.copying = false;
    if state
        .pending
        .as_ref()
        .is_some_and(|pending| pending.id == id)
    {
        state.pending = None;
    }
    inner.notifications.send_replace(state.snapshot.clone());
}

fn settled() -> SettledOperation {
    async {}.boxed().shared()
}
fn boxed_message(message: String) -> BoxError {
    Box::new(io::Error::other(message))
}
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
