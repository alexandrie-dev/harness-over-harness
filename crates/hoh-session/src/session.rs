use crate::store::{StoreInner, detach_session};
use crate::surface::{FoldBatchPlan, FoldState, validate_session_event_data};
use crate::*;
use hoh_kernel::Context;
use serde_json::{Value, json};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::ThreadId;

#[derive(Clone)]
pub struct Session(pub(crate) Arc<SessionInner>);
pub(crate) struct SessionInner {
    id: SessionId,
    header: Arc<Value>,
    inherited: SessionLogOffset,
    first_live: SessionLogOffset,
    first_lifecycle: SessionLogOffset,
    pub projections: MessageProjections,
    pub state: Mutex<SessionState>,
    section: Section,
}
pub(crate) struct SessionState {
    log: Arc<Vec<Arc<SessionEvent>>>,
    snapshot: Option<Arc<[Arc<SessionEvent>]>>,
    fold: Arc<FoldState>,
    pub owned: bool,
    pub attachment: Option<Attachment>,
    pub busy: bool,
    batch_publication: Option<Arc<SessionEvent>>,
    derived: Vec<Arc<Value>>,
    derived_nodes: usize,
    derived_generation: u64,
    header_fold: Option<Arc<Value>>,
    header_seq: usize,
    context_fold: Option<Arc<Value>>,
    context_seq: usize,
    tools: tool_history::ToolHistoryProjection,
    tools_seq: usize,
}
#[derive(Clone)]
pub(crate) struct Attachment {
    pub store: Weak<StoreInner>,
    pub ctx: Context,
    pub announcement: Announcement,
    pub detach_requested: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Announcement {
    Pending,
    Dispatching,
    Complete,
    Failed,
}
impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("id", self.id())
            .field("seq", &self.seq())
            .finish()
    }
}
impl PartialEq for Session {
    fn eq(&self, b: &Self) -> bool {
        Arc::ptr_eq(&self.0, &b.0)
    }
}
impl Eq for Session {}
impl hoh_scope::ScopeFilter for Session {
    fn matches_filter(&self, _: &Context) -> bool {
        true
    }
}
impl Session {
    pub fn create(
        id: SessionId,
        seed: Option<&[Arc<SessionEvent>]>,
        header: Option<Value>,
        inherited: Option<SessionLogOffset>,
        projections: Option<MessageProjections>,
    ) -> Result<Self> {
        Self::construct(id, seed, header, inherited, projections, false)
    }
    pub fn from_restore(
        id: SessionId,
        seed: &[Arc<SessionEvent>],
        header: Value,
        inherited: SessionLogOffset,
        _event_state: SessionSeedEventState,
        projections: Option<MessageProjections>,
    ) -> Result<Self> {
        Self::construct(
            id,
            Some(seed),
            Some(header),
            Some(inherited),
            projections,
            true,
        )
    }
    fn construct(
        id: SessionId,
        seed: Option<&[Arc<SessionEvent>]>,
        header: Option<Value>,
        inherited: Option<SessionLogOffset>,
        projections: Option<MessageProjections>,
        restore: bool,
    ) -> Result<Self> {
        let header = header.unwrap_or_else(
            || json!({"version":SESSION_FORMAT_VERSION,"id":id,"createdAt":now(),"isSeeded":false}),
        );
        if restore {
            validation::validate_header(&id, &header)?;
        }
        let projections = projections.unwrap_or_default();
        let definitions = lock(&projections).clone();
        let mut log: Vec<Arc<SessionEvent>> = vec![];
        let mut fold = FoldState::default();
        for (index, e) in seed.unwrap_or_default().iter().enumerate() {
            if !restore && !hoh_util_values::is_json_value(e.as_value()) {
                return Err(SessionError::message(format!(
                    "seed event at index {index} is not losslessly JSON-serializable"
                )));
            }
            validation::assert_envelope(e, index)?;
            if u64::from(e.seq()) != index as u64 {
                return Err(SessionError::message(format!(
                    "seed event at index {index} has seq {} (expected {index}); seed must be contiguous from 0",
                    e.seq()
                )));
            }
            fold.apply(
                e,
                SessionSeq::new(index as u64)?,
                &log,
                SessionLogOffset::default(),
                &definitions,
            )
            .map_err(|e| {
                SessionError::message(format!("invalid seed event at index {index}: {e}"))
            })?;
            log.push(if restore {
                e.clone()
            } else {
                Arc::new((**e).clone())
            });
        }
        if !restore && hoh_util_values::snapshot_json_value(&header).is_none() {
            return Err(SessionError::message(
                "session header is not losslessly JSON-serializable",
            ));
        }
        validation::validate_header(&id, &header)?;
        let first_live = SessionLogOffset::new(log.len() as u64)?;
        let seeded = header["isSeeded"] == true;
        if seeded && seed.is_none() {
            return Err(SessionError::message(
                "seeded session requires an explicit constructor seed",
            ));
        }
        if seeded && inherited.is_none() {
            return Err(SessionError::message(
                "seeded session requires an inherited event count",
            ));
        }
        let inherited = inherited.unwrap_or_default();
        if !seeded && u64::from(inherited) != 0 {
            return Err(SessionError::message(
                "unseeded session inherited event count must be 0",
            ));
        }
        if u64::from(inherited) > log.len() as u64 {
            return Err(SessionError::message(
                "session inherited event count exceeds its event log",
            ));
        }
        let tagged = |e: &Arc<SessionEvent>| {
            e.event_type() == "session/end-seed" && e.data()["inherited"] == true
        };
        let cut = u64::from(inherited) as usize;
        let marked = log.get(cut).is_some_and(tagged);
        if !restore && seeded && cut != log.len() && !marked {
            return Err(SessionError::message(
                "seeded session constructor seed must equal its inherited prefix or mark its inherited cut",
            ));
        }
        if marked && log.iter().skip(cut + 1).any(tagged) {
            return Err(SessionError::message(
                "session inherited event count must identify the final inherited marker",
            ));
        }
        let first_lifecycle = if !restore && seeded {
            inherited
        } else {
            first_live
        };
        let s = Self(Arc::new(SessionInner {
            id,
            header: Arc::new(header),
            inherited,
            first_live,
            first_lifecycle,
            projections,
            section: Section::default(),
            state: Mutex::new(SessionState {
                log: Arc::new(log),
                snapshot: None,
                fold: Arc::new(fold),
                owned: false,
                attachment: None,
                busy: false,
                batch_publication: None,
                derived: vec![],
                derived_nodes: 0,
                derived_generation: 0,
                header_fold: None,
                header_seq: 0,
                context_fold: None,
                context_seq: 0,
                tools: Default::default(),
                tools_seq: 0,
            }),
        }));
        if seed.is_some() && !restore && seeded && !marked {
            s.append("session/end-seed", json!({"inherited":true}), None)?;
        } else if seed.is_some()
            && (restore || !seeded)
            && s.snapshot_events(None, None)
                .last()
                .is_none_or(|e| e.event_type() != "session/end-seed")
        {
            s.append("session/end-seed", json!({}), None)?;
        }
        Ok(s)
    }
    pub fn id(&self) -> &SessionId {
        &self.0.id
    }
    pub fn header(&self) -> Arc<Value> {
        self.0.header.clone()
    }
    pub fn inherited_event_count(&self) -> SessionLogOffset {
        self.0.inherited
    }
    pub fn first_live_seq(&self) -> SessionLogOffset {
        self.0.first_live
    }
    pub fn first_lifecycle_seq(&self) -> SessionLogOffset {
        self.0.first_lifecycle
    }
    pub fn seq(&self) -> SessionLogOffset {
        // A Vec cannot reach the JavaScript safe-integer limit on supported platforms.
        SessionLogOffset::new(lock(&self.0.state).log.len() as u64)
            .expect("log length is a safe integer")
    }
    pub fn event_at(&self, seq: SessionSeq) -> Option<Arc<SessionEvent>> {
        lock(&self.0.state)
            .log
            .get(u64::from(seq) as usize)
            .cloned()
    }
    pub fn snapshot_events(
        &self,
        from: Option<SessionLogOffset>,
        to: Option<SessionLogOffset>,
    ) -> Arc<[Arc<SessionEvent>]> {
        let mut s = lock(&self.0.state);
        let from = u64::from(from.unwrap_or_default()) as usize;
        let to = to.map(|v| u64::from(v) as usize).unwrap_or(s.log.len());
        if from == 0 && to == s.log.len() {
            if let Some(snapshot) = &s.snapshot {
                return snapshot.clone();
            }
            let snapshot: Arc<[Arc<SessionEvent>]> = s.log.as_slice().to_vec().into();
            s.snapshot = Some(snapshot.clone());
            return snapshot;
        }
        let end = to.min(s.log.len());
        let start = from.min(end);
        s.log[start..end].to_vec().into()
    }
    pub fn own_events(&self) -> Arc<[Arc<SessionEvent>]> {
        self.snapshot_events(Some(self.inherited_event_count()), None)
    }
    pub fn is_own_seq(&self, seq: SessionSeq) -> bool {
        u64::from(seq) >= u64::from(self.0.inherited) && u64::from(seq) < u64::from(self.seq())
    }
    pub fn current_batch_publication(&self) -> Option<Arc<SessionEvent>> {
        lock(&self.0.state).batch_publication.clone()
    }
    pub fn surface(&self) -> Result<SessionSurface> {
        self.surface_snapshot()?;
        Ok(SessionSurface {
            session: self.clone(),
        })
    }
    pub(crate) fn surface_snapshot(&self) -> Result<surface::SurfaceSnapshot> {
        let p = lock(&self.0.projections).clone();
        let s = lock(&self.0.state);
        s.fold.assert_projections(&p)?;
        Ok(s.fold.surface.clone())
    }
    pub fn derive_event_message(&self, e: &SessionEvent) -> Result<Option<Arc<Value>>> {
        let p = lock(&self.0.projections).clone();
        let s = lock(&self.0.state);
        s.fold.assert_projections(&p)?;
        Ok(derive_event_message(e, Some(&s.fold.messages)))
    }
    pub fn derive_messages(&self) -> Result<Vec<Arc<Value>>> {
        let definitions = lock(&self.0.projections).clone();
        let mut s = lock(&self.0.state);
        s.fold.assert_projections(&definitions)?;
        if s.derived_generation != s.fold.surface.content_generation {
            s.derived.clear();
            s.derived_nodes = 0;
            s.derived_generation = s.fold.surface.content_generation;
        }
        let nodes = s.fold.surface.nodes[s.derived_nodes..].to_vec();
        for seq in nodes {
            if let Some(message) =
                derive_event_message(&s.log[u64::from(seq) as usize], Some(&s.fold.messages))
            {
                s.derived.push(message);
            }
        }
        s.derived_nodes = s.fold.surface.nodes.len();
        Ok(s.derived.clone())
    }
    pub fn request_header(&self) -> Option<Arc<Value>> {
        let mut s = lock(&self.0.state);
        if s.header_seq < s.log.len() {
            s.header_fold = fold_request_header(&s.log[s.header_seq..], s.header_fold.clone());
            s.header_seq = s.log.len();
        }
        s.header_fold.clone()
    }
    pub fn request_context(&self) -> Option<Arc<Value>> {
        let mut s = lock(&self.0.state);
        for i in s.context_seq..s.log.len() {
            if s.log[i].event_type() == "request/context" {
                s.context_fold = Some(Arc::new(s.log[i].data().clone()));
            }
        }
        s.context_seq = s.log.len();
        s.context_fold.clone()
    }
    pub fn tool_history(&self) -> Result<Arc<Value>> {
        let mut s = lock(&self.0.state);
        while s.tools_seq < s.log.len() {
            let e = s.log[s.tools_seq].clone();
            s.tools.apply(&e)?;
            s.tools_seq += 1;
        }
        Ok(s.tools.snapshot())
    }
    fn publication_entry(s: &SessionState, id: &SessionId) -> Result<Option<Attachment>> {
        if s.owned && s.attachment.is_none() {
            return Err(SessionError::message(format!(
                "session \"{id}\" cannot append outside a SessionStore"
            )));
        }
        if s.attachment
            .as_ref()
            .is_some_and(|a| a.announcement != Announcement::Complete)
        {
            return Err(SessionError::message(format!(
                "session \"{id}\" cannot append before session/created completes"
            )));
        }
        if s.busy {
            return Err(SessionError::message(
                "session append cannot reenter while another append is being published",
            ));
        }
        Ok(s.attachment.clone())
    }
    /// Run a synchronous sequence of appends without another thread's append in between,
    /// as code between two awaits is in the source.
    pub fn atomic<R>(&self, f: impl FnOnce() -> R) -> R {
        let _section = self.0.section.enter();
        f()
    }
    pub fn append(
        &self,
        t: &str,
        data: Value,
        opts: Option<SurfaceIntent>,
    ) -> Result<Arc<SessionEvent>> {
        let mut input = SessionEventInput::new(t, data);
        if let Some(opts) = opts {
            input = input.with_surface(opts);
        }
        let mut events = self.append_inputs(&[input], false, false)?;
        // A single input always commits exactly one event.
        Ok(events.remove(0))
    }
    pub fn append_batch(&self, inputs: &[SessionEventInput]) -> Result<Vec<Arc<SessionEvent>>> {
        self.append_inputs(inputs, true, false)
    }
    fn append_inputs(
        &self,
        inputs: &[SessionEventInput],
        batch: bool,
        record: bool,
    ) -> Result<Vec<Arc<SessionEvent>>> {
        let _section = self.0.section.enter();
        let definitions = lock(&self.0.projections).clone();
        // Declared before planning views: error unwinding releases them before detach callbacks.
        let mut guard = AppendGuard(None);
        let (entry, fold, log, start) = {
            let mut s = lock(&self.0.state);
            let entry = Self::publication_entry(&s, self.id())?;
            if inputs.is_empty() {
                return Ok(vec![]);
            }
            s.fold.assert_projections(&definitions)?;
            s.busy = true;
            guard.0 = Some(self.clone());
            (entry, s.fold.clone(), s.log.clone(), s.log.len())
        };
        let mut events = vec![];
        for (i, input) in inputs.iter().enumerate() {
            let subject = if record {
                format!("plugin record \"{}\"", input.r#type)
            } else if batch {
                format!("session batch member {i} (\"{}\")", input.r#type)
            } else {
                format!("session event \"{}\"", input.r#type)
            };
            let data = hoh_util_values::snapshot_json_value(&input.data).ok_or_else(|| {
                SessionError::message(format!("{subject} carries non-JSON-serializable data"))
            })?;
            let mut value =
                json!({"type":input.r#type,"seq":SessionSeq::new((start+i) as u64)?,"time":now()});
            // Move the detached payload; serializing Value again would copy it recursively.
            value["data"] = data;
            if record {
                value["ignorable"] = json!(true);
                if !is_plugin_record_type(&input.r#type) {
                    return Err(SessionError::message(format!(
                        "plugin record type \"{}\" must be \"plugin:\" followed by lowercase slash-separated segments",
                        input.r#type
                    )));
                }
            } else {
                for key in ["sourceEventSeqs", "surfaceOp"] {
                    if let Some(v) = input.opts.get(key) {
                        value[key] = hoh_util_values::snapshot_json_value(v).ok_or_else(|| {
                            SessionError::message(format!(
                                "{subject} carries non-JSON-serializable surface metadata"
                            ))
                        })?;
                    }
                }
            }
            let event = Arc::new(SessionEvent::from_value(value));
            events.push(event);
        }
        // Source snapshots every member before validating the first payload.
        for event in &events {
            if !record {
                validate_session_event_data(
                    event,
                    &format!(
                        "session event \"{}\" at seq {}",
                        event.event_type(),
                        event.seq()
                    ),
                )?;
            }
        }
        let plan = if batch {
            fold.plan_batch(&log, &events, SessionLogOffset::default(), &definitions)?
        } else {
            FoldBatchPlan::Deltas(vec![fold.plan(
                &events[0],
                SessionSeq::new(start as u64)?,
                &log,
                SessionLogOffset::default(),
                &definitions,
                None,
            )?])
        };
        let mut publications = vec![];
        if let Some(entry) = &entry {
            for e in &events {
                let payload = Arc::new(SessionEventEmitted::new(self.clone(), e.clone()));
                let callbacks = capture(|| {
                    entry
                        .ctx
                        .root()
                        .dispatch::<SessionEventEmitted>(payload.clone())
                })?;
                payload.validation.take()?;
                publications.push(callbacks);
            }
        }
        // No exported read retains these private planning views. Releasing them leaves
        // the committed owners unique, so push/commit need no copy-on-write traversal.
        drop(log);
        drop(fold);
        {
            let mut s = lock(&self.0.state);
            // Only the state owns the log after its planning view was released above.
            Arc::get_mut(&mut s.log)
                .expect("unique committed log")
                .extend_from_slice(&events);
            s.snapshot = None;
            // Only the state owns the fold after its planning view was released above.
            Arc::get_mut(&mut s.fold)
                .expect("unique committed fold")
                .commit_batch_plan(plan);
        }
        if let Some(entry) = entry {
            for (e, callbacks) in events.iter().zip(publications) {
                if batch {
                    lock(&self.0.state).batch_publication = Some(e.clone());
                }
                let log = entry.ctx.logger();
                let id = self.id().clone();
                callbacks.invoke(move |failure| {
                    events::report_observer(&log, &id, "session/event", failure)
                });
            }
        }
        drop(guard);
        Ok(events)
    }
}
/// Stand-in for the JavaScript guarantee that code between two awaits is indivisible: the owner
/// thread may append any number of times, every other thread's append waits for the section to end.
/// A nested append on the owner thread still meets the `busy` check ("cannot reenter").
#[derive(Default)]
struct Section {
    owner: Mutex<(Option<ThreadId>, usize)>,
    free: Condvar,
}
struct SectionGuard<'a>(&'a Section);
impl Section {
    fn enter(&self) -> SectionGuard<'_> {
        let me = std::thread::current().id();
        let mut owner = lock(&self.owner);
        while owner.0.is_some_and(|other| other != me) {
            owner = self
                .free
                .wait(owner)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        owner.0 = Some(me);
        owner.1 += 1;
        SectionGuard(self)
    }
}
impl Drop for SectionGuard<'_> {
    fn drop(&mut self) {
        let mut owner = lock(&self.0.owner);
        owner.1 -= 1;
        if owner.1 == 0 {
            owner.0 = None;
            self.0.free.notify_all();
        }
    }
}
struct AppendGuard(Option<Session>);
impl Drop for AppendGuard {
    fn drop(&mut self) {
        let Some(session) = self.0.take() else {
            return;
        };
        let detach = {
            let mut s = lock(&session.0.state);
            s.busy = false;
            s.batch_publication = None;
            s.attachment.as_ref().is_some_and(|a| a.detach_requested)
        };
        if detach {
            detach_session(&session);
        }
    }
}
pub fn append_plugin_record(session: &Session, t: &str, data: Value) -> Result<SessionSeq> {
    Ok(session.append_inputs(&[SessionEventInput::new(t, data)], false, true)?[0].seq())
}
fn is_plugin_record_type(t: &str) -> bool {
    t.strip_prefix("plugin:").is_some_and(|rest| {
        rest.split('/').all(|segment| {
            let mut b = segment.bytes();
            b.next()
                .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                && b.all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-')
                })
        })
    })
}
pub fn plugin_record_of(e: &SessionEvent) -> Option<PluginRecord> {
    if !e.ignorable() || !is_plugin_record_type(e.event_type()) {
        return None;
    }
    Some(PluginRecord {
        r#type: e.event_type().into(),
        seq: e.seq(),
        time: e.time(),
        data: e.data().clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn rejects_invalid_raw_fork_watermarks_without_panicking() {
        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        let store = SessionStore::new();
        for (index, raw) in [json!(-1), json!(null), json!("invalid")]
            .into_iter()
            .enumerate()
        {
            for requested in [None, Some(SessionSeq::new(0).unwrap())] {
                let source = store
                    .create(
                        &ctx,
                        Some(format!("damaged-{index}-{requested:?}").into()),
                        Default::default(),
                    )
                    .unwrap();
                source
                    .append("turn/start", json!({"turn":1}), None)
                    .unwrap();
                {
                    let mut state = lock(&source.0.state);
                    let mut value = state.log[0].as_value().clone();
                    value["seq"] = raw.clone();
                    Arc::make_mut(&mut state.log)[0] = Arc::new(SessionEvent::from_value(value));
                }
                let error = store
                    .fork(&ctx, SessionForkSource::Session(source), requested, None)
                    .unwrap_err();
                assert_eq!(
                    error.downcast_ref::<SessionForkError>().unwrap().code,
                    SessionForkErrorCode::InvalidBoundary
                );
                assert!(error.to_string().contains(if requested.is_none() {
                    "non-negative safe integer"
                } else {
                    "does not match a contiguous event seq"
                }));
            }
        }
        kernel.shutdown().await;
    }
    #[tokio::test(start_paused = true)]
    async fn rejects_a_corrupted_live_source_whose_array_index_no_longer_matches_event_seq() {
        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        let store = SessionStore::new();
        let source = store
            .create(&ctx, Some("corrupt-parent".into()), Default::default())
            .unwrap();
        for kind in ["turn/start", "user/message", "turn/end"] {
            let data = if kind == "user/message" {
                json!({"id":"m","role":"user","content":[],"source":{"kind":"user"}})
            } else {
                json!({"turn":1,"reason":{"kind":"completed"}})
            };
            source
                .append(
                    kind,
                    data,
                    if kind == "user/message" {
                        Some(SurfaceIntent::append())
                    } else {
                        None
                    },
                )
                .unwrap();
        }
        {
            let mut state = lock(&source.0.state);
            let mut event = state.log[2].as_value().clone();
            event["seq"] = json!(99);
            Arc::make_mut(&mut state.log)[2] = Arc::new(SessionEvent::from_value(event));
            state.snapshot = None;
        }
        let error = store
            .fork(
                &ctx,
                SessionForkSource::Session(source),
                Some(SessionSeq::new(2).unwrap()),
                Some("corrupt-child".into()),
            )
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "fork boundary 2 does not match a contiguous event seq in session \"corrupt-parent\""
        );
        assert_eq!(
            error.downcast_ref::<SessionForkError>().unwrap().code,
            SessionForkErrorCode::InvalidBoundary
        );
        assert!(store.get(&"corrupt-child".into()).is_none());
        kernel.shutdown().await;
    }
}
