use crate::session::{Announcement, Attachment};
use crate::*;
use hoh_error::BoxError;
use hoh_kernel::{Context, Disposer, ListenerFailure, Undo};
use indexmap::IndexMap;
use serde_json::json;
use std::{
    future::Future,
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex},
};
#[derive(Clone)]
pub struct SessionStore {
    inner: Arc<StoreInner>,
}
pub(crate) struct StoreInner {
    state: Mutex<StoreState>,
    projections: MessageProjections,
}
#[derive(Default)]
struct StoreState {
    entries: IndexMap<SessionId, Session>,
    counter: u64,
}
impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}
impl SessionStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(StoreInner {
                state: Mutex::new(StoreState::default()),
                projections: Default::default(),
            }),
        }
    }
    pub fn message_projections(&self) -> MessageProjections {
        self.inner.projections.clone()
    }
    pub fn register_message_projection(
        &self,
        ctx: &Context,
        p: Arc<dyn SessionMessageProjection>,
    ) -> Result<Disposer> {
        let definitions = self.inner.projections.clone();
        ctx.try_effect(
            "sessions.registerMessageProjection()",
            move |undos| -> Result<()> {
                {
                    let mut list = lock(&definitions);
                    if list.iter().any(|v| v.event_type() == p.event_type()) {
                        return Err(SessionError::message(format!(
                            "session message projection \"{}\" is already registered",
                            p.event_type()
                        )));
                    }
                    list.push(p.clone());
                }
                undos.push(Undo::sync(move || {
                    lock(&definitions).retain(|v| !Arc::ptr_eq(v, &p));
                }))?;
                Ok(())
            },
        )
    }
    pub fn prepare(
        &self,
        id: Option<SessionId>,
        options: PrepareSessionOptions,
    ) -> Result<Session> {
        let id = {
            let mut state = lock(&self.inner.state);
            if let Some(id) = id {
                id
            } else {
                loop {
                    state.counter += 1;
                    let id = SessionId::from(format!("session-{}", state.counter));
                    if !state.entries.contains_key(&id) {
                        break id;
                    }
                }
            }
        };
        if self.get(&id).is_some() {
            return Err(SessionError::message(format!(
                "session \"{id}\" already exists"
            )));
        }
        let session = match options {
            PrepareSessionOptions::Restored(o) => Session::from_restore(
                id,
                &o.seed,
                o.meta,
                o.inherited_event_count,
                o.event_state,
                Some(self.inner.projections.clone()),
            )?,
            PrepareSessionOptions::Create(o) => {
                let meta = o.meta.unwrap_or_else(|| json!({}));
                let mut header = json!({"version":SESSION_FORMAT_VERSION,"id":id,"createdAt":meta.get("createdAt").filter(|v|!v.is_null()).cloned().unwrap_or_else(||json!(now()))});
                for key in ["cwd", "parentSession"] {
                    if let Some(v) = meta.get(key) {
                        header[key] = v.clone();
                    }
                }
                header["isSeeded"] = meta
                    .get("isSeeded")
                    .filter(|v| !v.is_null())
                    .cloned()
                    .unwrap_or(json!(false));
                for key in ["origin", "delegationDepth", "agentPreset"] {
                    if let Some(v) = meta.get(key) {
                        header[key] = v.clone();
                    }
                }
                Session::create(
                    id,
                    o.seed.as_deref(),
                    Some(header),
                    o.inherited_event_count,
                    Some(self.inner.projections.clone()),
                )?
            }
        };
        lock(&session.0.state).owned = true;
        Ok(session)
    }
    pub fn create(
        &self,
        ctx: &Context,
        id: Option<SessionId>,
        options: CreateSessionOptions,
    ) -> Result<Session> {
        let session = self.prepare(id, PrepareSessionOptions::Create(options))?;
        let s = session.clone();
        ctx.try_effect("sessions.create()", |undos| -> Result<()> {
            let detach = self.enter(ctx, s.clone())?;
            undos.push(Undo::sync(move || detach.detach()))?;
            self.announce(&s)
        })?;
        Ok(session)
    }
    pub fn enter(&self, ctx: &Context, session: Session) -> Result<SessionDetach> {
        let mut store = lock(&self.inner.state);
        if store.entries.contains_key(session.id()) {
            return Err(SessionError::message(format!(
                "session \"{}\" already exists",
                session.id()
            )));
        }
        {
            let mut state = lock(&session.0.state);
            if state.attachment.is_some() {
                return Err(SessionError::message(format!(
                    "session \"{}\" is already attached to a store",
                    session.id()
                )));
            }
            state.owned = true;
            state.attachment = Some(Attachment {
                store: Arc::downgrade(&self.inner),
                ctx: ctx.clone(),
                announcement: Announcement::Pending,
                detach_requested: false,
            });
        }
        store.entries.insert(session.id().clone(), session.clone());
        Ok(SessionDetach {
            session,
            _store: self.inner.clone(),
            called: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }
    fn live_entry_for(&self, s: &Session) -> Result<Attachment> {
        let live = lock(&self.inner.state)
            .entries
            .get(s.id())
            .is_some_and(|v| v == s);
        let entry = lock(&s.0.state).attachment.clone();
        if live
            && let Some(entry) = entry
            && entry.store.ptr_eq(&Arc::downgrade(&self.inner))
        {
            return Ok(entry);
        }
        Err(SessionError::message(format!(
            "session \"{}\" is not live in this store",
            s.id()
        )))
    }
    pub fn announce(&self, s: &Session) -> Result<()> {
        let entry = self.live_entry_for(s)?;
        {
            let mut state = lock(&s.0.state);
            let a = state
                .attachment
                .as_mut()
                .ok_or_else(|| SessionError::message("session detached"))?;
            if a.announcement != Announcement::Pending {
                return Err(SessionError::message(format!(
                    "session \"{}\" was already announced",
                    s.id()
                )));
            }
            a.announcement = Announcement::Dispatching;
        }
        let payload = Arc::new(SessionCreated {
            session: s.clone(),
            validation: Default::default(),
            veto: Mutex::new(None),
        });
        let result = (|| {
            let callbacks =
                capture(|| entry.ctx.root().dispatch::<SessionCreated>(payload.clone()))?;
            payload.validation.take()?;
            let p = payload.clone();
            let log = entry.ctx.logger();
            let id = s.id().clone();
            callbacks.invoke(move |failure| match failure {
                ListenerFailure::Threw(error) => {
                    let mut veto = lock(&p.veto);
                    if veto.is_none() {
                        *veto = Some(error);
                    }
                }
                failure => events::report_observer(&log, &id, "session/created", failure),
            });
            let veto = lock(&payload.veto).take();
            veto.map_or(Ok(()), Err)
        })();
        let detach = {
            let mut state = lock(&s.0.state);
            if let Some(a) = state.attachment.as_mut() {
                a.announcement = if result.is_ok() {
                    Announcement::Complete
                } else {
                    Announcement::Failed
                };
                a.detach_requested
            } else {
                false
            }
        };
        if detach {
            detach_session(s);
        }
        result
    }
    /// Fallible synchronous creation observers stop at the first veto.
    pub fn on_created(
        ctx: &Context,
        f: impl Fn(&Session) -> Result<()> + Send + Sync + 'static,
    ) -> Result<Disposer> {
        Ok(ctx.on_contained::<SessionCreated>(move |e| {
            if lock(&e.veto).is_some() {
                return Ok(());
            }
            match capture(|| f(&e.session)) {
                Ok(Ok(())) => {}
                Ok(Err(error)) | Err(error) => {
                    *lock(&e.veto) = Some(error);
                }
            }
            Ok(())
        })?)
    }
    pub fn on_flush<F, Fut>(ctx: &Context, f: F) -> Result<Disposer>
    where
        F: Fn(Session) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        Ok(ctx.on_contained::<SessionFlush>(move |e| {
            e.wait_for(f(e.session.clone()));
            Ok(())
        })?)
    }
    pub async fn flush(&self, s: &Session) -> Result<bool> {
        use futures_util::FutureExt;
        let entry = self.live_entry_for(s)?;
        let payload = Arc::new(SessionFlush {
            session: s.clone(),
            validation: Default::default(),
            pending: Mutex::new(vec![]),
        });
        let callbacks = capture(|| entry.ctx.root().dispatch::<SessionFlush>(payload.clone()))?;
        payload.validation.take()?;
        let participated = !callbacks.is_empty();
        let p = payload.clone();
        callbacks.invoke(move |f| {
            let error = match f {
                ListenerFailure::Threw(e) | ListenerFailure::Rejected(e) => e,
            };
            p.wait_for(async move { Err(error) });
        });
        let futures = std::mem::take(&mut *lock(&payload.pending));
        let results = futures_util::future::join_all(futures.into_iter().map(|f| async move {
            AssertUnwindSafe(f)
                .catch_unwind()
                .await
                .map_err(panic_error)
                .and_then(|r| r)
        }))
        .await;
        for result in results {
            result?;
        }
        Ok(participated)
    }
    /// Async creation failures are diagnostic only and cannot veto publication.
    pub fn on_created_async<F, Fut>(ctx: &Context, f: F) -> Result<Disposer>
    where
        F: Fn(Session) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        Ok(ctx.on_contained_async::<SessionCreated, _>(move |e| {
            let future = if lock(&e.veto).is_none() {
                Some(f(e.session.clone()))
            } else {
                None
            };
            async move {
                match future {
                    Some(future) => future.await,
                    None => Ok(()),
                }
            }
        })?)
    }
    pub fn get(&self, id: &SessionId) -> Option<Session> {
        lock(&self.inner.state).entries.get(id).cloned()
    }
    pub fn list(&self) -> Vec<Session> {
        lock(&self.inner.state).entries.values().cloned().collect()
    }
    pub fn fork(
        &self,
        ctx: &Context,
        source: SessionForkSource,
        boundary: Option<SessionSeq>,
        child_id: Option<SessionId>,
    ) -> Result<Session> {
        if let Some(id) = &child_id
            && self.get(id).is_some()
        {
            return Err(SessionForkError::boxed(
                format!("session \"{id}\" already exists"),
                SessionForkErrorCode::SessionAlreadyExists,
            ));
        }
        let source = match source {
            SessionForkSource::Id(id) => self.get(&id).ok_or_else(|| {
                SessionForkError::boxed(
                    format!("session \"{id}\" not found"),
                    SessionForkErrorCode::SessionNotFound,
                )
            })?,
            SessionForkSource::Session(s) => {
                let live = self.get(s.id()).ok_or_else(|| {
                    SessionForkError::boxed(
                        format!("session \"{}\" not found", s.id()),
                        SessionForkErrorCode::SessionNotFound,
                    )
                })?;
                if live != s {
                    return Err(SessionForkError::boxed(
                        format!("session \"{}\" is not the live store instance", s.id()),
                        SessionForkErrorCode::SessionNotLive,
                    ));
                }
                s
            }
        };
        let events = source.snapshot_events(None, None);
        let position = |event: &SessionEvent| {
            event
                .as_value()
                .get("seq")
                .and_then(crate::types::signed_number)
                .filter(|n| *n >= 0)
                .and_then(|n| SessionSeq::new(n as u64).ok())
        };
        let resolved = if let Some(boundary) = boundary {
            Some(boundary)
        } else if let Some(last) = events.last() {
            Some(position(last).ok_or_else(|| SessionForkError::boxed(
                format!("fork boundary for session \"{}\" must be a non-negative safe integer, got {}",
                    source.id(), crate::validation::js_field(last.as_value().get("seq"))),
                SessionForkErrorCode::InvalidBoundary,
            ))?)
        } else {
            None
        };
        if let Some(boundary) = resolved
            && events.get(u64::from(boundary) as usize).is_none()
        {
            return Err(SessionForkError::boxed(
                format!(
                    "fork boundary {boundary} does not exist in session \"{}\" (last seq: {})",
                    source.id(),
                    events
                        .last()
                        .and_then(|e| e.as_value().get("seq"))
                        .filter(|v| !v.is_null())
                        .map(|v| crate::validation::js_field(Some(v)))
                        .unwrap_or_else(|| "none".into())
                ),
                SessionForkErrorCode::InvalidBoundary,
            ));
        }
        if let Some(boundary) = resolved
            && events
                .get(u64::from(boundary) as usize)
                .is_some_and(|e| position(e) != Some(boundary))
        {
            return Err(SessionForkError::boxed(
                format!(
                    "fork boundary {boundary} does not match a contiguous event seq in session \"{}\"",
                    source.id()
                ),
                SessionForkErrorCode::InvalidBoundary,
            ));
        }
        let seed = resolved
            .map(|b| build_fork_seed(&events, b))
            .transpose()?
            .unwrap_or_default();
        let inherited = SessionLogOffset::new(resolved.map_or(0, |b| u64::from(b) + 1))?;
        let mut meta = json!({});
        if let Some(cwd) = source.header().get("cwd") {
            meta["cwd"] = cwd.clone();
        }
        meta["isSeeded"] = json!(true);
        self.create(
            ctx,
            child_id,
            CreateSessionOptions {
                seed: Some(seed),
                inherited_event_count: Some(inherited),
                meta: Some(meta),
            },
        )
    }
}
#[derive(Clone)]
pub struct SessionDetach {
    session: Session,
    _store: Arc<StoreInner>,
    called: Arc<std::sync::atomic::AtomicBool>,
}
impl SessionDetach {
    pub fn detach(&self) {
        if self.called.swap(true, std::sync::atomic::Ordering::AcqRel) {
            return;
        }
        {
            let mut s = lock(&self.session.0.state);
            let defer = s.busy
                || s.attachment
                    .as_ref()
                    .is_some_and(|a| a.announcement == Announcement::Dispatching);
            if defer {
                if let Some(a) = s.attachment.as_mut() {
                    a.detach_requested = true;
                }
                return;
            }
        }
        detach_session(&self.session);
    }
    pub fn into_undo(self) -> Undo {
        Undo::sync(move || self.detach())
    }
}
pub(crate) fn detach_session(s: &Session) {
    let entry = lock(&s.0.state).attachment.clone();
    let Some(entry) = entry else {
        return;
    };
    let Some(store) = entry.store.upgrade() else {
        lock(&s.0.state).attachment = None;
        return;
    };
    {
        let mut state = lock(&store.state);
        if state.entries.get(s.id()).is_none_or(|v| v != s) {
            return;
        }
        state.entries.shift_remove(s.id());
        lock(&s.0.state).attachment = None;
    }
    if entry.announcement != Announcement::Pending {
        let log = entry.ctx.logger();
        let id = s.id().clone();
        let payload = Arc::new(SessionDisposed {
            session: s.clone(),
            validation: Default::default(),
        });
        match capture(|| {
            let callbacks = entry
                .ctx
                .root()
                .dispatch::<SessionDisposed>(payload.clone());
            payload.validation.take()?;
            Ok(callbacks)
        })
        .and_then(|r| r)
        {
            Ok(callbacks) => {
                callbacks.invoke(move |f| events::report_observer(&log, &id, "session/disposed", f))
            }
            Err(error) => log.warn(format_args!(
                "session \"{id}\": session/disposed dispatch threw: Error: {error}"
            )),
        }
    }
}
#[derive(Clone, Debug)]
pub enum SessionForkSource {
    Id(SessionId),
    Session(Session),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionForkErrorCode {
    SessionNotFound,
    SessionNotLive,
    SessionAlreadyExists,
    InvalidBoundary,
}
impl SessionForkErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionNotFound => "SESSION_NOT_FOUND",
            Self::SessionNotLive => "SESSION_NOT_LIVE",
            Self::SessionAlreadyExists => "SESSION_ALREADY_EXISTS",
            Self::InvalidBoundary => "INVALID_BOUNDARY",
        }
    }
}
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct SessionForkError {
    pub message: String,
    pub code: SessionForkErrorCode,
}
impl SessionForkError {
    fn boxed(message: String, code: SessionForkErrorCode) -> BoxError {
        Box::new(Self { message, code })
    }
    pub fn name(&self) -> &'static str {
        "SessionForkError"
    }
}
hoh_kernel::service!(pub Sessions: SessionStore = "sessions");
pub struct SessionPlugin;
impl hoh_kernel::Plugin for SessionPlugin {
    const NAME: &'static str = "SessionStore";
    type Config = hoh_kernel::NoConfig;
    type Deps = ();
    async fn apply(
        ctx: Context,
        _: (),
        _: hoh_kernel::Cfg<Self::Config>,
    ) -> std::result::Result<(), hoh_kernel::Error> {
        ctx.provide::<Sessions>(Arc::new(SessionStore::new()))
            .await?;
        Ok(())
    }
}
