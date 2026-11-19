use crate::{
    ContextPressureProjection, ContextPressureState, MeasurementState, MeterSurfaceNode,
    TokenMeasurement, TokenMeasurementBaseline, advance_measurement, commit_surface_tokens, error,
    estimate_message, estimate_tools_tokens, measurement_projection_definition,
    plan_surface_tokens, price_surface,
};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_llm::{Llm, Message};
use hoh_session::{
    EpochHeader, Session, SessionEvent, SessionEventEmitted, SessionLogOffset, SessionSeq,
    canonical_header, header_equals, is_surface_event,
};
use hoh_session_projection::{ProjectionHandle, SessionProjections};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, MutexGuard, Weak},
};

#[derive(Clone)]
struct CachedHeader {
    seq: SessionSeq,
    value: EpochHeader,
}

#[derive(Default)]
struct ReplayState {
    consumed_events: SessionLogOffset,
    surface: Vec<MeterSurfaceNode>,
    header: Option<CachedHeader>,
    anchor_header: Option<CachedHeader>,
    anchor_nodes: Vec<MeterSurfaceNode>,
    pending_measurement: MeasurementState,
}

struct StateEntry {
    identity: Weak<Value>,
    state: ReplayState,
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn session_identity(session: &Session) -> (usize, Weak<Value>) {
    let header = session.header();
    (Arc::as_ptr(&header) as usize, Arc::downgrade(&header))
}

fn optional_header_equals(left: Option<&EpochHeader>, right: Option<&EpochHeader>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => header_equals(left, right),
        _ => false,
    }
}

fn number(value: &Value, key: &str) -> Option<f64> {
    value.get(key)?.as_f64()
}

fn usage_tokens(usage: &Value) -> Option<f64> {
    Some(
        number(usage, "inputTokens")?
            + number(usage, "cacheReadTokens").unwrap_or(0.0)
            + number(usage, "cacheWriteTokens").unwrap_or(0.0)
            + number(usage, "outputTokens")?,
    )
}

fn advance_local_measurement(state: &MeasurementState, event: &SessionEvent) -> MeasurementState {
    match advance_measurement(state, event) {
        Ok(next) => next,
        Err(_) => MeasurementState {
            invalid_seq: Some(event.seq()),
            ..state.clone()
        },
    }
}

fn validate_measurement_event(
    measurement: &MeasurementState,
    surface: &[MeterSurfaceNode],
    event: &SessionEvent,
) -> Result<(), BoxError> {
    if measurement.invalid_seq == Some(event.seq()) {
        if event.event_type() == "assistant/message" {
            let _ = plan_surface_tokens(surface, event)?;
        }
        advance_measurement(measurement, event)?;
    }
    Ok(())
}

type FileText = dyn Fn(&hoh_attachment::FileAttachmentRef) -> String + Send + Sync;

hoh_kernel::service!(pub TokenMeter: TokenMeterRuntime = "tokenMeter");

/// Replay owner for isolated per-session folds.
pub struct TokenMeterRuntime {
    _ctx: Context,
    measurement: Option<ProjectionHandle<MeasurementState>>,
    pressure: Option<ProjectionHandle<ContextPressureState>>,
    states: Mutex<HashMap<usize, StateEntry>>,
}

impl TokenMeterRuntime {
    /// Construct the source class face before asynchronous projection activation.
    /// `measure` uses its identical local pure fold until registrations exist.
    pub fn new(ctx: Context) -> Self {
        Self {
            _ctx: ctx,
            measurement: None,
            pressure: None,
            states: Mutex::new(HashMap::new()),
        }
    }

    fn with_projections(
        ctx: Context,
        measurement: ProjectionHandle<MeasurementState>,
        pressure: ProjectionHandle<ContextPressureState>,
    ) -> Self {
        Self {
            _ctx: ctx,
            measurement: Some(measurement),
            pressure: Some(pressure),
            states: Mutex::new(HashMap::new()),
        }
    }

    /// Read the latest logged provider prompt size and route capacity.
    pub fn context_pressure(
        &self,
        session: &Session,
    ) -> Result<ContextPressureProjection, BoxError> {
        let state = self
            .pressure
            .as_ref()
            .ok_or_else(|| error("token meter: context pressure is unavailable before activation"))?
            .state_of(session)?;
        Ok(ContextPressureProjection {
            pressure_tokens: state.pressure_tokens,
            projected_tokens: None,
            context_window: state.context_window,
        })
    }

    /// Heuristically price one model-visible message.
    pub fn estimate_message(&self, message: &Message) -> u64 {
        estimate_message(message)
    }

    /// Measure current request pressure and surface through the durable tail.
    pub fn measure(
        &self,
        caller: &Context,
        session: &Session,
        request_header: Option<&EpochHeader>,
    ) -> Result<TokenMeasurement, BoxError> {
        let (state, measurement) = self.sync_snapshot(session)?;
        let header = request_header
            .map(canonical_header)
            .or_else(|| Self::cached_header(&state, measurement.header_seq).cloned());
        let llm = caller.get::<Llm>();
        let pricing = header
            .as_ref()
            .and_then(|header| header.get("config"))
            .and_then(|config| {
                Some((
                    config.get("provider")?.as_str()?,
                    config.get("model")?.as_str()?,
                ))
            })
            .and_then(|(provider, model)| llm.as_ref()?.image_request_pricing(provider, model));
        let file_text: Option<Box<FileText>> = llm.as_ref().map(|llm| {
            let llm = llm.clone();
            let ctx = caller.clone();
            Box::new(move |reference: &hoh_attachment::FileAttachmentRef| {
                llm.file_request_text(&ctx, reference)
            }) as _
        });
        let surface = price_surface(&state.surface, pricing.as_deref(), file_text.as_deref())?;
        let anchor = measurement.anchor.as_ref();
        let (baseline, surface_delta_tokens) = if let Some(anchor) = anchor.filter(|anchor| {
            optional_header_equals(
                Self::cached_header(&state, anchor.header_seq),
                header.as_ref(),
            )
        }) {
            let anchor_surface_tokens = price_surface(
                &state.anchor_nodes,
                pricing.as_deref(),
                file_text.as_deref(),
            )?
            .surface_tokens as f64
                + anchor.assistant_tokens;
            let estimated = estimate_tools_tokens(header.as_ref()) as f64 + anchor_surface_tokens;
            let baseline = anchor
                .usage
                .as_ref()
                .and_then(usage_tokens)
                .filter(|tokens| *tokens >= estimated)
                .map_or(
                    TokenMeasurementBaseline::Estimated { tokens: estimated },
                    |tokens| TokenMeasurementBaseline::Usage {
                        tokens,
                        usage: anchor.usage.clone().expect("usage was present"),
                    },
                );
            (
                baseline,
                surface.surface_tokens as f64 - anchor_surface_tokens,
            )
        } else if header.is_none() && surface.surface_tokens == 0 {
            (TokenMeasurementBaseline::None { tokens: 0.0 }, 0.0)
        } else {
            (
                TokenMeasurementBaseline::Estimated {
                    tokens: estimate_tools_tokens(header.as_ref())
                        .saturating_add(surface.surface_tokens) as f64,
                },
                0.0,
            )
        };
        Ok(TokenMeasurement {
            log_revision: state.consumed_events,
            total_tokens: (baseline.tokens() + surface_delta_tokens).max(0.0),
            baseline,
            surface_delta_tokens,
            surface_tokens: surface.surface_tokens,
            nodes: surface.nodes,
        })
    }

    fn cached_header(state: &ReplayState, seq: Option<SessionSeq>) -> Option<&EpochHeader> {
        let seq = seq?;
        if state
            .header
            .as_ref()
            .is_some_and(|header| header.seq == seq)
        {
            return state.header.as_ref().map(|header| &header.value);
        }
        state.anchor_header.as_ref().map(|header| &header.value)
    }

    fn observed(&self, session: &Session) -> bool {
        let (key, weak) = session_identity(session);
        lock(&self.states).get(&key).is_some_and(|entry| {
            Weak::ptr_eq(&entry.identity, &weak) && entry.identity.strong_count() > 0
        })
    }

    fn sync_if_observed(&self, session: &Session) -> Result<(), BoxError> {
        if self.observed(session) {
            self.sync_snapshot(session)?;
        }
        Ok(())
    }

    fn sync_snapshot(
        &self,
        session: &Session,
    ) -> Result<(ReplayState, MeasurementState), BoxError> {
        let (key, identity) = session_identity(session);
        let mut states = lock(&self.states);
        states.retain(|_, entry| entry.identity.strong_count() > 0);
        let replace = states
            .get(&key)
            .is_some_and(|entry| !Weak::ptr_eq(&entry.identity, &identity));
        if replace {
            states.remove(&key);
        }
        let entry = states.entry(key).or_insert_with(|| StateEntry {
            identity,
            state: ReplayState::default(),
        });
        let mut use_projection = true;
        loop {
            let projected = if use_projection {
                self.measurement
                    .as_ref()
                    .map(|handle| handle.state_of(session))
                    .transpose()?
            } else {
                None
            };
            while entry.state.consumed_events < session.seq() {
                let seq = SessionSeq::new(entry.state.consumed_events.get())?;
                let event = session.event_at(seq).ok_or_else(|| {
                    error(format!(
                        "token meter: session is missing event at seq {}",
                        seq.get()
                    ))
                })?;
                let next = if projected.is_none() {
                    Some(advance_local_measurement(
                        &entry.state.pending_measurement,
                        &event,
                    ))
                } else {
                    None
                };
                let measurement = next.as_ref().or(projected.as_deref());
                if let Some(measurement) = measurement {
                    validate_measurement_event(measurement, &entry.state.surface, &event)?;
                }
                Self::fold_event(&mut entry.state, &event)?;
                if let Some(next) = next {
                    entry.state.pending_measurement = next;
                }
                entry.state.consumed_events =
                    SessionLogOffset::new(entry.state.consumed_events.get() + 1)?;
            }
            if let Some(projected) = &projected {
                let current = entry.state.header.as_ref().map(|header| header.seq);
                let anchor = entry.state.anchor_header.as_ref().map(|header| header.seq);
                if projected.header_seq != current
                    || projected
                        .anchor
                        .as_ref()
                        .is_some_and(|projected| projected.header_seq != anchor)
                {
                    entry.state = ReplayState::default();
                    use_projection = false;
                    continue;
                }
            }
            let measurement = projected
                .as_deref()
                .cloned()
                .unwrap_or_else(|| entry.state.pending_measurement.clone());
            return Ok((entry.state.clone(), measurement));
        }
    }

    fn fold_event(state: &mut ReplayState, event: &SessionEvent) -> Result<(), BoxError> {
        if event.event_type() == "request/header" {
            let header = event
                .data()
                .get("header")
                .ok_or_else(|| error("token meter: request/header is missing header"))?;
            state.header = Some(CachedHeader {
                seq: event.seq(),
                value: canonical_header(header),
            });
        }
        if event.event_type() == "image/offload" {
            let mut targets: HashMap<SessionSeq, HashSet<usize>> = HashMap::new();
            for target in event
                .data()
                .get("targets")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(seq) = target
                    .get("seq")
                    .and_then(super::surface_projection::safe_count)
                    .and_then(|seq| SessionSeq::new(seq).ok())
                else {
                    continue;
                };
                let indexes = target
                    .get("imageIndexes")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(super::surface_projection::safe_count)
                    .filter_map(|index| usize::try_from(index).ok())
                    .collect();
                targets.insert(seq, indexes);
            }
            for node in &mut state.surface {
                let Some(indexes) = targets.get(&node.seq) else {
                    continue;
                };
                for (index, image) in node.images.iter_mut().enumerate() {
                    if indexes.contains(&index) {
                        image.offloaded = Some(true);
                    }
                }
            }
        }
        let plan = is_surface_event(event)
            .then(|| plan_surface_tokens(&state.surface, event))
            .transpose()?;
        if event.event_type() == "assistant/message" {
            state.anchor_nodes = state.surface.clone();
            state.anchor_header = state.header.clone();
        }
        if let Some(plan) = plan {
            commit_surface_tokens(&mut state.surface, plan);
        }
        Ok(())
    }
}

impl Clone for ReplayState {
    fn clone(&self) -> Self {
        Self {
            consumed_events: self.consumed_events,
            surface: self.surface.clone(),
            header: self.header.clone(),
            anchor_header: self.anchor_header.clone(),
            anchor_nodes: self.anchor_nodes.clone(),
            pending_measurement: self.pending_measurement.clone(),
        }
    }
}

#[derive(Inject)]
pub struct Deps {
    projections: Service<SessionProjections>,
}

pub struct TokenMeterPlugin;

impl Plugin for TokenMeterPlugin {
    const NAME: &'static str = "TokenMeter";
    type Config = crate::TokenMeterConfig;
    type Deps = Deps;

    async fn apply(
        ctx: Context,
        deps: Deps,
        config: Cfg<crate::TokenMeterConfig>,
    ) -> Result<(), BoxError> {
        if let Some(key) = config.get().unknown_key() {
            return Err(error(format!(
                "TokenMeterConfig: unknown key \"{key}\" (no settings are supported)"
            )));
        }
        let measurement = deps
            .projections
            .register(&ctx, measurement_projection_definition())
            .await?;
        deps.projections
            .register(&ctx, crate::token_usage_projection_definition())
            .await?;
        let pressure = deps
            .projections
            .register(&ctx, crate::context_pressure_projection_definition())
            .await?;
        deps.projections
            .register(&ctx, crate::context_breakdown_projection_definition())
            .await?;
        let runtime = Arc::new(TokenMeterRuntime::with_projections(
            ctx.clone(),
            measurement,
            pressure,
        ));
        let observer = runtime.clone();
        ctx.on_contained::<SessionEventEmitted>(move |event| {
            observer.sync_if_observed(&event.session)
        })?;
        ctx.provide::<TokenMeter>(runtime).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoh_kernel::Kernel;
    use serde_json::json;

    #[tokio::test]
    async fn dead_session_cache_entries_are_pruned_before_the_next_observation() {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        let runtime = TokenMeterRuntime::new(ctx.clone());
        {
            let first = Session::create("first".into(), None, None, None, None).unwrap();
            runtime.measure(&ctx, &first, None).unwrap();
            assert_eq!(lock(&runtime.states).len(), 1);
        }
        let second = Session::create("second".into(), None, None, None, None).unwrap();
        runtime.measure(&ctx, &second, None).unwrap();
        assert_eq!(lock(&runtime.states).len(), 1);
    }

    fn event(seq: u64, kind: &str, data: Value, surface_op: Option<Value>) -> SessionEvent {
        let mut value = json!({"seq":seq,"time":0,"type":kind,"data":data});
        if let Some(surface_op) = surface_op {
            value["surfaceOp"] = surface_op;
        }
        SessionEvent::from_value(value)
    }

    fn user(text: &str) -> Value {
        json!({
            "role":"user","content":[{"type":"text","text":text}],
            "source":{"kind":"user"},"id":"u"
        })
    }

    #[test]
    fn local_replay_reports_an_assistant_surface_range_before_its_lifecycle_failure() {
        let assistant = event(
            1,
            "assistant/message",
            json!({
                "turn":1,"step":1,"stream":[],
                "message":{"role":"assistant","content":[],"source":{"kind":"model","provider":"p","model":"m"},"id":"a"}
            }),
            Some(json!({"op":"replace","startSeq":99,"endSeq":99})),
        );
        let measurement = advance_local_measurement(&MeasurementState::default(), &assistant);
        assert_eq!(measurement.invalid_seq, Some(assistant.seq()));
        assert_eq!(
            validate_measurement_event(&measurement, &[], &assistant)
                .unwrap_err()
                .to_string(),
            "token surface: replace at seq 1 has invalid current range 99-99"
        );
    }

    #[test]
    fn lifecycle_failure_does_not_commit_a_valid_assistant_replacement() {
        let head = event(0, "user/message", user("head"), Some(json!("append")));
        let mut surface = Vec::new();
        let plan = plan_surface_tokens(&surface, &head).unwrap();
        commit_surface_tokens(&mut surface, plan);
        let before = surface.clone();
        let assistant = event(
            1,
            "assistant/message",
            json!({
                "turn":1,"step":1,"stream":[],
                "message":{"role":"assistant","content":[],"source":{"kind":"model","provider":"p","model":"m"},"id":"a"}
            }),
            Some(json!({"op":"replace","startSeq":0,"endSeq":0})),
        );
        let measurement = advance_local_measurement(&MeasurementState::default(), &assistant);
        assert_eq!(
            validate_measurement_event(&measurement, &surface, &assistant)
                .unwrap_err()
                .to_string(),
            "token meter: assistant/message at seq 1 has no matching step/start event"
        );
        assert_eq!(surface, before);
    }

    #[test]
    fn lifecycle_failure_does_not_commit_a_valid_assistant_append() {
        let assistant = event(
            0,
            "assistant/message",
            json!({
                "turn":1,"step":1,"stream":[],
                "message":{"role":"assistant","content":[{"type":"text","text":"planned but never committed"}],"source":{"kind":"model","provider":"p","model":"m"},"id":"a"}
            }),
            Some(json!("append")),
        );
        let state = ReplayState::default();
        let measurement = advance_local_measurement(&state.pending_measurement, &assistant);
        assert_eq!(
            validate_measurement_event(&measurement, &state.surface, &assistant)
                .unwrap_err()
                .to_string(),
            "token meter: assistant/message at seq 0 has no matching step/start event"
        );
        assert!(state.surface.is_empty());
        assert_eq!(state.consumed_events, SessionLogOffset::default());
    }

    #[test]
    fn corrupt_surface_replay_is_repeatable_and_does_not_advance_its_cursor() {
        let corrupt = event(
            1,
            "user/message",
            user("bad"),
            Some(json!({"op":"replace","startSeq":99,"endSeq":99})),
        );
        let mut state = ReplayState::default();
        for _ in 0..2 {
            assert_eq!(
                TokenMeterRuntime::fold_event(&mut state, &corrupt)
                    .unwrap_err()
                    .to_string(),
                "token surface: replace at seq 1 has invalid current range 99-99"
            );
            assert_eq!(state.consumed_events, SessionLogOffset::default());
            assert!(state.surface.is_empty());
        }
    }
}
