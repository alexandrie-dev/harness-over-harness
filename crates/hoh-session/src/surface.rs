//! Canonical surface validation and plugin-owned message projection.
use crate::known_event_types::{KNOWN_SESSION_EVENT_TYPES, MESSAGE_PROJECTION_EVENT_TYPES};
use crate::types::seq_number;
use crate::{Result, SessionError, SessionEvent, SessionLogOffset, SessionSeq, SurfaceOp, lock};
use indexmap::IndexMap;
use serde_json::Value;
use std::sync::{Arc, Mutex};

pub fn is_surface_eligible_type(t: &str) -> bool {
    matches!(
        t,
        "system/message"
            | "developer/message"
            | "user/message"
            | "assistant/message"
            | "tool/result"
    )
}
pub fn is_surface_event(e: &SessionEvent) -> bool {
    is_surface_eligible_type(e.event_type()) && e.surface_op().is_some()
}
pub fn is_append_surface_event(e: &SessionEvent) -> bool {
    is_surface_event(e) && e.surface_op() == Some(&Value::String("append".into()))
}
pub fn is_replacement_surface_event(e: &SessionEvent) -> bool {
    is_surface_event(e) && !is_append_surface_event(e)
}

pub fn derive_event_message(
    e: &SessionEvent,
    projected: Option<&IndexMap<SessionSeq, Arc<Value>>>,
) -> Option<Arc<Value>> {
    if let Some(message) = projected.and_then(|m| m.get(&e.seq())) {
        return Some(message.clone());
    }
    let message = e.message()?;
    if matches!(
        e.event_type(),
        "system/message" | "developer/message" | "assistant/message"
    ) && message["content"].as_array().is_some_and(|c| c.is_empty())
    {
        return None;
    }
    e.message_snapshot()
}
pub fn validate_session_event_data(e: &SessionEvent, subject: &str) -> Result<()> {
    let data = e.data();
    let fail = |text: &str| SessionError::message(format!("{subject} {text}"));
    if is_surface_eligible_type(e.event_type()) && data.is_object() {
        let message = if e.event_type() == "user/message" {
            data
        } else {
            &data["message"]
        };
        if message.is_object() {
            let developer = e.event_type() == "developer/message";
            if developer != (message["role"] == "developer") {
                return Err(fail(
                    "developer/message and developer role must occur together",
                ));
            }
            if let Some(content) = message["content"].as_array() {
                if !developer && content.iter().any(is_tool_change) {
                    return Err(fail("tool-change blocks require developer role"));
                }
                if developer {
                    let mut additions = false;
                    for block in content.iter().filter(|b| is_tool_change(b)) {
                        if !nonempty(&block["toolName"]) {
                            return Err(fail(&format!(
                                "{} requires a nonempty toolName",
                                block["type"].as_str().unwrap_or("")
                            )));
                        }
                        if block["type"] == "tool-addition" {
                            additions = true;
                            if block.get("tool").is_some() {
                                return Err(fail(
                                    "tool-addition must omit inline tool definitions",
                                ));
                            }
                        }
                    }
                    if if additions {
                        data.get("headerSeq").and_then(seq_number).is_none()
                    } else {
                        data.get("headerSeq").is_some()
                    } {
                        return Err(fail(
                            "requires headerSeq exactly when tool additions are present",
                        ));
                    }
                }
            }
        }
    }
    if e.event_type() == "request/header" {
        if !data.is_object() {
            return Err(fail("data must be an object"));
        }
        let header = &data["header"];
        if !header.is_object() {
            return Err(fail("header must be an object"));
        }
        if header.get("system").is_some() {
            return Err(fail("must omit header.system; use system/message"));
        }
        if header["tools"].as_array().is_some_and(|a| a.is_empty()) {
            return Err(fail("must omit empty tools"));
        }
        if header["adapterDefaults"]
            .as_object()
            .is_some_and(|a| a.is_empty())
        {
            return Err(fail("must omit empty adapterDefaults"));
        }
    } else if e.event_type() == "tool/result" {
        if !data.is_object() {
            return Err(fail("data must be an object"));
        }
        if data.get("error").is_some() && data["message"]["isError"] != true {
            return Err(fail("error requires message.isError === true"));
        }
    }
    Ok(())
}
fn is_tool_change(v: &Value) -> bool {
    v.is_object() && matches!(v["type"].as_str(), Some("tool-addition" | "tool-removal"))
}
pub(crate) fn nonempty(v: &Value) -> bool {
    v.as_str().is_some_and(|s| !s.is_empty())
}

pub fn validate_surface_metadata(e: &SessionEvent) -> Result<Option<SurfaceOp>> {
    let t = e.event_type();
    if !is_surface_eligible_type(t) {
        if !KNOWN_SESSION_EVENT_TYPES.contains(&t) && e.ignorable() {
            return Ok(None);
        }
        for key in ["surfaceOp", "sourceEventSeqs"] {
            if e.as_value().get(key).is_some() {
                return Err(SessionError::message(format!(
                    "session event \"{t}\" is not surface-eligible and cannot carry {key}"
                )));
            }
        }
        return Ok(None);
    }
    let raw = e.surface_op().ok_or_else(|| {
        SessionError::message(format!(
            "session event \"{t}\" is surface-eligible and requires a surfaceOp marker"
        ))
    })?;
    let op = if raw == "append" {
        SurfaceOp::append()
    } else {
        let msg = if raw.is_object() {
            "invalid replace surfaceOp"
        } else {
            "invalid surfaceOp"
        };
        serde_json::from_value::<SurfaceOp>(raw.clone())
            .map_err(|_| SessionError::message(format!("session event \"{t}\" carries an {msg}")))?
    };
    if let SurfaceOp::Replace(ref r) = op
        && (r.start_seq >= e.seq() || r.end_seq >= e.seq())
    {
        return Err(SessionError::message(format!(
            "surface replace at seq {}: startSeq and endSeq must reference earlier events",
            e.seq()
        )));
    }
    assert_source_event_references(e, &[])?;
    Ok(Some(op))
}
fn assert_source_event_references(e: &SessionEvent, shadowed: &[SessionSeq]) -> Result<()> {
    let mut sources = indexmap::IndexSet::new();
    if let Some(raw) = e.source_event_seqs() {
        if e.event_type() == "assistant/message" {
            return Err(SessionError::message(
                "assistant/message embeds its source stream and cannot carry sourceEventSeqs",
            ));
        }
        let array = raw.as_array().ok_or_else(|| {
            SessionError::message(format!(
                "sourceEventSeqs on event at seq {} must be an array when present",
                e.seq()
            ))
        })?;
        if array.is_empty() {
            return Err(SessionError::message("sourceEventSeqs must not be empty"));
        }
        let mut non_earlier = None;
        for v in array {
            let n = seq_number(v).ok_or_else(||SessionError::message(format!("session event \"{}\" sourceEventSeqs must densely contain non-negative safe integers", e.event_type())))?;
            sources.insert(SessionSeq::new(n)?);
            if non_earlier.is_none() && n >= u64::from(e.seq()) {
                non_earlier = Some(n);
            }
        }
        if sources.len() != array.len() {
            return Err(SessionError::message(
                "sourceEventSeqs must not contain duplicates",
            ));
        }
        if let Some(n) = non_earlier {
            return Err(SessionError::message(format!(
                "sourceEventSeqs must reference earlier events: {n} >= current seq {}",
                e.seq()
            )));
        }
    }
    let missing = shadowed
        .iter()
        .filter(|s| !sources.contains(*s))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(SessionError::message(format!(
            "surface replace: sourceEventSeqs must include every shadowed surface node; missing {}",
            missing.join(", ")
        )));
    }
    Ok(())
}
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceFoldReplacement {
    pub seq: SessionSeq,
    pub start: SessionSeq,
    pub end: SessionSeq,
    pub shadowed_seqs: Vec<SessionSeq>,
}
#[derive(Clone, Debug, Default)]
pub struct SurfaceFoldResult {
    pub nodes: Vec<SessionSeq>,
    pub replacements: Vec<SurfaceFoldReplacement>,
    pub projected_messages: IndexMap<SessionSeq, Arc<Value>>,
}
#[derive(Clone, Debug, Default)]
pub struct SurfaceSnapshot {
    pub nodes: Vec<SessionSeq>,
    pub replace_generation: u64,
    pub content_generation: u64,
}
/// A stable, read-only live view. Holding it keeps the underlying log alive;
/// accessors read the latest committed state without lending a mutex guard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSurface {
    pub(crate) session: crate::Session,
}
impl SessionSurface {
    pub fn nodes(&self) -> Result<Vec<SessionSeq>> {
        Ok(self.session.surface_snapshot()?.nodes)
    }
    pub fn replace_generation(&self) -> Result<u64> {
        Ok(self.session.surface_snapshot()?.replace_generation)
    }
    pub fn content_generation(&self) -> Result<u64> {
        Ok(self.session.surface_snapshot()?.content_generation)
    }
}
pub struct SessionMessageProjectionContext<'a> {
    pub nodes: &'a [SessionSeq],
    pub events: &'a [Arc<SessionEvent>],
    pub base_seq: SessionLogOffset,
    pub messages: &'a IndexMap<SessionSeq, Arc<Value>>,
}
pub trait SessionMessageProjection: Send + Sync + 'static {
    fn event_type(&self) -> &str;
    fn project(
        &self,
        event: &SessionEvent,
        context: SessionMessageProjectionContext<'_>,
    ) -> Result<IndexMap<SessionSeq, Arc<Value>>>;
}
pub type MessageProjections = Arc<Mutex<Vec<Arc<dyn SessionMessageProjection>>>>;
#[derive(Clone, Default)]
pub(crate) struct FoldState {
    pub surface: SurfaceSnapshot,
    pub messages: IndexMap<SessionSeq, Arc<Value>>,
    used: Vec<Arc<dyn SessionMessageProjection>>,
}
/// A validated delta; planning never mutates the committed surface.
#[derive(Clone)]
pub(crate) enum SurfacePlan {
    None,
    Append(SessionSeq),
    Project {
        projection: Arc<dyn SessionMessageProjection>,
        messages: IndexMap<SessionSeq, Arc<Value>>,
    },
    Replace {
        replacement: SurfaceFoldReplacement,
        start_index: usize,
        end_index: usize,
    },
}
pub(crate) enum FoldBatchPlan {
    Deltas(Vec<SurfacePlan>),
    Isolated(FoldState),
}
impl FoldState {
    pub(crate) fn assert_projections(
        &self,
        projections: &[Arc<dyn SessionMessageProjection>],
    ) -> Result<()> {
        for p in &self.used {
            if !projections.iter().any(|v| Arc::ptr_eq(p, v)) {
                return Err(SessionError::message(format!(
                    "session message projection \"{}\" was removed or replaced; restore the session with its owning plugin",
                    p.event_type()
                )));
            }
        }
        Ok(())
    }
    pub(crate) fn apply(
        &mut self,
        e: &SessionEvent,
        expected: SessionSeq,
        events: &[Arc<SessionEvent>],
        base: SessionLogOffset,
        projections: &[Arc<dyn SessionMessageProjection>],
    ) -> Result<Option<SurfaceFoldReplacement>> {
        let plan = self.plan(e, expected, events, base, projections, None)?;
        Ok(self.commit(plan))
    }
    pub(crate) fn plan(
        &self,
        e: &SessionEvent,
        expected: SessionSeq,
        events: &[Arc<SessionEvent>],
        base: SessionLogOffset,
        projections: &[Arc<dyn SessionMessageProjection>],
        candidates: Option<&[Arc<SessionEvent>]>,
    ) -> Result<SurfacePlan> {
        if e.seq() != expected {
            return Err(SessionError::message(format!(
                "session event seq {} is not contiguous; expected {expected}",
                e.seq()
            )));
        }
        let op = validate_surface_metadata(e)?;
        let read = |seq: SessionSeq| {
            u64::from(seq).checked_sub(u64::from(base)).and_then(|n| {
                let index = n as usize;
                if index < events.len() {
                    events.get(index)
                } else {
                    candidates.and_then(|items| items.get(index - events.len()))
                }
            })
        };
        assert_developer_header(e, read)?;
        if let Some(p) = projections
            .iter()
            .find(|p| p.event_type() == e.event_type())
        {
            // Source only materializes log+candidates for a batch projection's events view.
            let combined;
            let projection_events = if let Some(candidates) = candidates {
                combined = events.iter().chain(candidates).cloned().collect::<Vec<_>>();
                combined.as_slice()
            } else {
                events
            };
            let messages = p.project(
                e,
                SessionMessageProjectionContext {
                    nodes: &self.surface.nodes,
                    events: projection_events,
                    base_seq: base,
                    messages: &self.messages,
                },
            )?;
            return Ok(SurfacePlan::Project {
                projection: p.clone(),
                messages,
            });
        }
        if MESSAGE_PROJECTION_EVENT_TYPES.contains(&e.event_type()) {
            return Err(SessionError::message(format!(
                "session event \"{}\" requires a message projection; load its owning plugin or supply its projection definition",
                e.event_type()
            )));
        }
        match op {
            None => Ok(SurfacePlan::None),
            Some(SurfaceOp::Append(_)) => Ok(SurfacePlan::Append(e.seq())),
            Some(SurfaceOp::Replace(op)) => {
                let start = self
                    .surface
                    .nodes
                    .iter()
                    .position(|s| *s == op.start_seq)
                    .ok_or_else(|| {
                        SessionError::message(format!(
                            "surface replace: start seq {} not found in surface",
                            op.start_seq
                        ))
                    })?;
                let end = self
                    .surface
                    .nodes
                    .iter()
                    .position(|s| *s == op.end_seq)
                    .ok_or_else(|| {
                        SessionError::message(format!(
                            "surface replace: end seq {} not found in surface",
                            op.end_seq
                        ))
                    })?;
                if start > end {
                    return Err(SessionError::message(format!(
                        "surface replace: start seq {} (index {start}) is after end seq {} (index {end})",
                        op.start_seq, op.end_seq
                    )));
                }
                let shadowed = self.surface.nodes[start..=end].to_vec();
                assert_source_event_references(e, &shadowed)?;
                if e.event_type() == "tool/result" {
                    if shadowed.len() != 1 {
                        return Err(SessionError::message(
                            "tool/result surface replacement must rewrite exactly one current node",
                        ));
                    }
                    let old = read(shadowed[0])
                        .filter(|v| v.event_type() == "tool/result")
                        .ok_or_else(|| {
                            SessionError::message(
                                "tool/result surface replacement must target a current tool/result",
                            )
                        })?;
                    let mut a = old.data().clone();
                    let mut b = e.data().clone();
                    a["message"]["content"] = Value::Null;
                    b["message"]["content"] = Value::Null;
                    if !hoh_util_values::deep_equal_json(&a, &b) {
                        return Err(SessionError::message(
                            "tool/result surface replacement may change only content",
                        ));
                    }
                }
                if start == 0
                    && read(shadowed[0]).is_some_and(|v| v.event_type() == "system/message")
                    && (e.event_type() != "system/message" || shadowed.len() != 1)
                {
                    return Err(SessionError::message(
                        "surface replace: node 0 holds the system prompt and may be rewritten only by a system/message over exactly that node",
                    ));
                }
                Ok(SurfacePlan::Replace {
                    replacement: SurfaceFoldReplacement {
                        seq: e.seq(),
                        start: op.start_seq,
                        end: op.end_seq,
                        shadowed_seqs: shadowed,
                    },
                    start_index: start,
                    end_index: end,
                })
            }
        }
    }
    pub(crate) fn commit(&mut self, plan: SurfacePlan) -> Option<SurfaceFoldReplacement> {
        match plan {
            SurfacePlan::None => {}
            SurfacePlan::Append(seq) => self.surface.nodes.push(seq),
            SurfacePlan::Project {
                projection,
                messages,
            } => {
                self.messages.extend(messages);
                if !self.used.iter().any(|used| Arc::ptr_eq(used, &projection)) {
                    self.used.push(projection);
                }
                self.surface.content_generation += 1;
            }
            SurfacePlan::Replace {
                replacement,
                start_index,
                end_index,
            } => {
                self.surface
                    .nodes
                    .splice(start_index..=end_index, [replacement.seq]);
                self.surface.replace_generation += 1;
                self.surface.content_generation += 1;
                return Some(replacement);
            }
        }
        None
    }
    pub(crate) fn plan_batch(
        &self,
        events: &[Arc<SessionEvent>],
        candidates: &[Arc<SessionEvent>],
        base: SessionLogOffset,
        projections: &[Arc<dyn SessionMessageProjection>],
    ) -> Result<FoldBatchPlan> {
        self.assert_projections(projections)?;
        let simple = candidates.iter().all(|event| {
            event.surface_op().is_none_or(|op| op == "append")
                && !projections
                    .iter()
                    .any(|p| p.event_type() == event.event_type())
        });
        if simple {
            let plans = candidates
                .iter()
                .enumerate()
                .map(|(index, event)| {
                    self.plan(
                        event,
                        SessionSeq::new(u64::from(base) + (events.len() + index) as u64)?,
                        events,
                        base,
                        projections,
                        Some(candidates),
                    )
                })
                .collect::<Result<Vec<_>>>()?;
            return Ok(FoldBatchPlan::Deltas(plans));
        }
        // Source prepareBatch copies the fold only for replacements or projections.
        let mut isolated = self.clone();
        for (index, event) in candidates.iter().enumerate() {
            let plan = isolated.plan(
                event,
                SessionSeq::new(u64::from(base) + (events.len() + index) as u64)?,
                events,
                base,
                projections,
                Some(candidates),
            )?;
            isolated.commit(plan);
        }
        Ok(FoldBatchPlan::Isolated(isolated))
    }
    pub(crate) fn commit_batch_plan(&mut self, plan: FoldBatchPlan) {
        match plan {
            FoldBatchPlan::Deltas(plans) => {
                for plan in plans {
                    self.commit(plan);
                }
            }
            FoldBatchPlan::Isolated(state) => *self = state,
        }
    }
}
fn assert_developer_header<'a>(
    e: &SessionEvent,
    read: impl Fn(SessionSeq) -> Option<&'a Arc<SessionEvent>>,
) -> Result<()> {
    if e.event_type() != "developer/message" {
        return Ok(());
    }
    validate_session_event_data(e, &format!("developer/message at seq {}", e.seq()))?;
    let Some(raw) = e.data().get("headerSeq") else {
        return Ok(());
    };
    // Event-local validation above proved headerSeq is a safe integer.
    let seq = SessionSeq::new(seq_number(raw).expect("validated developer header reference"))?;
    let header = read(seq)
        .filter(|h| seq < e.seq() && h.event_type() == "request/header")
        .ok_or_else(|| {
            SessionError::message(
                "developer/message headerSeq must reference an earlier request/header",
            )
        })?;
    if let Some(content) = e.data()["message"]["content"].as_array() {
        for block in content.iter().filter(|b| b["type"] == "tool-addition") {
            let definitions = header.data()["header"]["tools"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|t| t["name"] == block["toolName"])
                .collect::<Vec<_>>();
            let name = block["toolName"].as_str().unwrap_or("");
            if definitions.len() != 1 {
                return Err(SessionError::message(format!(
                    "developer/message tool-addition \"{name}\" must name exactly one tool in headerSeq {seq}"
                )));
            }
            let d = definitions[0];
            if !d["description"].is_string() || !d["parameters"].is_object() {
                return Err(SessionError::message(format!(
                    "developer/message tool-addition \"{name}\" requires a complete tool definition in headerSeq {seq}"
                )));
            }
            if d.get("deferLoading").is_some_and(|v| v != true) {
                return Err(SessionError::message(
                    "developer/message referenced tool deferLoading must be true when present",
                ));
            }
        }
    }
    Ok(())
}
pub fn fold_surface(
    events: &[Arc<SessionEvent>],
    projections: &[Arc<dyn SessionMessageProjection>],
) -> Result<SurfaceFoldResult> {
    let mut state = FoldState::default();
    let mut replacements = vec![];
    for (index, e) in events.iter().enumerate() {
        if let Some(r) = state.apply(
            e,
            SessionSeq::new(index as u64)?,
            events,
            SessionLogOffset::default(),
            projections,
        )? {
            replacements.push(r);
        }
    }
    Ok(SurfaceFoldResult {
        nodes: state.surface.nodes,
        replacements,
        projected_messages: state.messages,
    })
}
/// Explicit loaded-window owner. `validate_next` plans without changing accepted state;
/// `commit_batch` adopts only a complete successful plan.
pub struct SurfaceManager {
    log: Vec<Arc<SessionEvent>>,
    base_seq: SessionLogOffset,
    projections: MessageProjections,
    state: FoldState,
    pending: Mutex<Option<(SessionEvent, FoldState)>>,
}
pub struct PreparedSurfaceBatch {
    state: FoldState,
    events: Vec<Arc<SessionEvent>>,
    expected_len: usize,
}
impl PreparedSurfaceBatch {
    pub fn commit(self, manager: &mut SurfaceManager) -> Result<()> {
        if manager.log.len() != self.expected_len {
            return Err(SessionError::message(
                "surface batch no longer follows its prepared log",
            ));
        }
        manager.log.extend(self.events);
        manager.state = self.state;
        *lock(&manager.pending) = None;
        Ok(())
    }
}
impl SurfaceManager {
    pub fn new(
        log: Vec<Arc<SessionEvent>>,
        base_seq: SessionLogOffset,
        projections: MessageProjections,
    ) -> Result<Self> {
        let mut state = FoldState::default();
        let definitions = lock(&projections).clone();
        for (i, e) in log.iter().enumerate() {
            state.apply(
                e,
                SessionSeq::new(u64::from(base_seq) + i as u64)?,
                &log,
                base_seq,
                &definitions,
            )?;
        }
        Ok(Self {
            log,
            base_seq,
            projections,
            state,
            pending: Mutex::new(None),
        })
    }
    pub fn surface(&self) -> Result<SurfaceSnapshot> {
        self.state.assert_projections(&lock(&self.projections))?;
        Ok(self.state.surface.clone())
    }
    pub fn derive_event_message(&self, e: &SessionEvent) -> Result<Option<Arc<Value>>> {
        self.state.assert_projections(&lock(&self.projections))?;
        Ok(derive_event_message(e, Some(&self.state.messages)))
    }
    pub fn validate_next(&self, e: &SessionEvent) -> Result<()> {
        let mut s = self.state.clone();
        let definitions = lock(&self.projections).clone();
        s.assert_projections(&definitions)?;
        s.apply(
            e,
            SessionSeq::new(u64::from(self.base_seq) + self.log.len() as u64)?,
            &self.log,
            self.base_seq,
            &definitions,
        )?;
        *lock(&self.pending) = Some((e.clone(), s));
        Ok(())
    }
    pub fn prepare_batch(&self, events: &[Arc<SessionEvent>]) -> Result<PreparedSurfaceBatch> {
        let projections = lock(&self.projections).clone();
        self.state.assert_projections(&projections)?;
        if let [event] = events
            && let Some((candidate, state)) = lock(&self.pending).as_ref()
            && candidate == event.as_ref()
        {
            state.assert_projections(&projections)?;
            return Ok(PreparedSurfaceBatch {
                state: state.clone(),
                events: events.to_vec(),
                expected_len: self.log.len(),
            });
        }
        let mut state = self.state.clone();
        let mut log = self.log.clone();
        log.extend_from_slice(events);
        for (i, e) in events.iter().enumerate() {
            state.apply(
                e,
                SessionSeq::new(u64::from(self.base_seq) + (self.log.len() + i) as u64)?,
                &log,
                self.base_seq,
                &projections,
            )?;
        }
        Ok(PreparedSurfaceBatch {
            state,
            events: events.to_vec(),
            expected_len: self.log.len(),
        })
    }
    pub fn commit_batch(&mut self, events: &[Arc<SessionEvent>]) -> Result<()> {
        self.prepare_batch(events)?.commit(self)
    }
}
