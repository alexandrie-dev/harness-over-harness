//! Incremental, byte-bounded session-log contribution for official DeepSeek requests.

pub mod types;
pub use types::*;

use std::sync::{
    Arc, Mutex, MutexGuard, OnceLock, Weak,
    atomic::{AtomicU64, Ordering},
};

use futures_util::FutureExt;
use hoh_deepseek_llm_api_extensions::{
    AcceptanceCallback, DeepSeekLlmApiExtensionRequest, DeepSeekLlmApiExtensions,
    PreparedDeepSeekLlmApiExtension,
};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Disposer, Inject, Plugin, Service, Undo, Volatile};
use hoh_session::{
    KNOWN_SESSION_EVENT_TYPES, Session, SessionEvent, SessionHeader, SessionLogOffset, SessionSeq,
    SessionSeqCursor, Sessions,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const DEFAULT_MAX_BYTES: f64 = (8 * 1024 * 1024) as f64;
const DELIVERY_ACCEPTED: &str = "session-log-deepseek/delivery-accepted";

/// Cordis-compatible package name.
pub const NAME: &str = "session-log-deepseek";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn error(message: impl Into<String>) -> BoxError {
    message.into().into()
}

fn default_enabled() -> Volatile<bool> {
    Volatile(true)
}

fn default_max_bytes() -> f64 {
    DEFAULT_MAX_BYTES
}

/// Session-log request contribution configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default = "default_enabled")]
    pub enabled: Volatile<bool>,
    #[serde(default = "default_max_bytes")]
    pub max_bytes: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            max_bytes: default_max_bytes(),
        }
    }
}

fn resolve_max_bytes(value: f64) -> Result<usize, BoxError> {
    if !value.is_finite() || value < 1.0 || value.fract() != 0.0 || value > usize::MAX as f64 {
        return Err(error(
            "session-log-deepseek: maxBytes must be a positive integer",
        ));
    }
    Ok(value as usize)
}

#[derive(Clone, Copy, Default)]
struct AcceptanceFold {
    scanned_events: SessionLogOffset,
    through_seq: SessionSeqCursor,
}

struct CachedAcceptanceFold {
    header: Weak<Value>,
    fold: AcceptanceFold,
}

static ACCEPTANCE_FOLDS: OnceLock<Mutex<Vec<CachedAcceptanceFold>>> = OnceLock::new();

fn cursor_number(cursor: SessionSeqCursor) -> i64 {
    match cursor {
        SessionSeqCursor::BeforeFirst => -1,
        SessionSeqCursor::Seq(seq) => {
            i64::try_from(u64::from(seq)).expect("safe sequence fits i64")
        }
    }
}

fn parse_safe_nonnegative(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => {
            if let Some(value) = number.as_u64() {
                return (value <= hoh_session::MAX_SAFE_INTEGER).then_some(value);
            }
            let value = number.as_f64()?;
            if value.is_finite()
                && value >= 0.0
                && value.fract() == 0.0
                && !(value == 0.0 && value.is_sign_negative())
                && value <= hoh_session::MAX_SAFE_INTEGER as f64
            {
                Some(value as u64)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn fold_acceptances(
    session: &Session,
    previous: AcceptanceFold,
) -> Result<AcceptanceFold, BoxError> {
    let length = session.seq();
    let mut through_seq = previous.through_seq;
    for index in u64::from(previous.scanned_events)..u64::from(length) {
        let seq = SessionSeq::new(index)?;
        let event = session.event_at(seq).ok_or_else(|| {
            error(format!(
                "session-log-deepseek: missing event {index} below captured length {length}"
            ))
        })?;
        if event.event_type() != DELIVERY_ACCEPTED {
            continue;
        }
        let data = event.data();
        let format = match data.get("sessionFormatVersion") {
            None | Some(Value::Null) => 0,
            Some(value) => parse_safe_nonnegative(value).ok_or_else(|| {
                error(format!(
                    "session-log-deepseek: malformed acceptance format version at seq {}",
                    event.seq()
                ))
            })?,
        };
        let header_version = session.header()["version"]
            .as_u64()
            .ok_or_else(|| error("session-log-deepseek: malformed Session header version"))?;
        if format != header_version {
            continue;
        }
        let accepted = data
            .get("throughSeq")
            .and_then(parse_safe_nonnegative)
            .and_then(|value| SessionSeq::new(value).ok())
            .ok_or_else(|| {
                error(format!(
                    "session-log-deepseek: malformed acceptance watermark at seq {}",
                    event.seq()
                ))
            })?;
        let session_id_valid = data
            .get("sessionId")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty());
        if !session_id_valid || accepted >= event.seq() {
            return Err(error(format!(
                "session-log-deepseek: malformed acceptance watermark at seq {}",
                event.seq()
            )));
        }
        if data["sessionId"].as_str() != Some(session.id().to_string().as_str()) {
            continue;
        }
        let advances = match through_seq {
            SessionSeqCursor::BeforeFirst => true,
            SessionSeqCursor::Seq(current) => accepted > current,
        };
        if advances {
            through_seq = SessionSeqCursor::Seq(accepted);
        }
    }
    Ok(AcceptanceFold {
        scanned_events: length,
        through_seq,
    })
}

/// Highest confirmed sequence for this exact Session format generation.
///
/// A weak header identity provides the source WeakMap semantics without
/// retaining the Session or its event log.
pub fn accepted_through(session: &Session) -> Result<SessionSeqCursor, BoxError> {
    let header = session.header();
    let weak = Arc::downgrade(&header);
    let cache = ACCEPTANCE_FOLDS.get_or_init(|| Mutex::new(Vec::new()));
    let mut cache = lock(cache);
    cache.retain(|entry| entry.header.strong_count() != 0);
    let previous = cache
        .iter()
        .find(|entry| Weak::ptr_eq(&entry.header, &weak))
        .map_or(AcceptanceFold::default(), |entry| entry.fold);
    let fold = fold_acceptances(session, previous)?;
    if let Some(entry) = cache
        .iter_mut()
        .find(|entry| Weak::ptr_eq(&entry.header, &weak))
    {
        entry.fold = fold;
    } else {
        cache.push(CachedAcceptanceFold { header: weak, fold });
    }
    Ok(fold.through_seq)
}

fn wire_header(session: &Session) -> Result<DeepSeekSessionLogWireHeader, BoxError> {
    let header: SessionHeader = serde_json::from_value((*session.header()).clone())?;
    Ok(DeepSeekSessionLogWireHeader {
        version: header.version,
        id: header.id.to_string(),
        created_at: header.created_at,
        cwd: header.cwd,
        parent_session: header.parent_session.map(|id| id.to_string()),
        seed_length: header
            .is_seeded
            .then(|| u64::from(session.inherited_event_count())),
        origin: header.origin,
        delegation_depth: header.delegation_depth,
        agent_preset: header.agent_preset,
    })
}

fn wire_event(event: &SessionEvent) -> DeepSeekSessionLogWireEvent {
    let event_type = event.event_type().to_owned();
    let surface = matches!(
        event_type.as_str(),
        "developer/message" | "system/message" | "user/message" | "tool/result"
    );
    let assistant = event_type == "assistant/message";
    let opaque = !KNOWN_SESSION_EVENT_TYPES.contains(&event_type.as_str()) && event.ignorable();
    DeepSeekSessionLogWireEvent {
        seq: u64::from(event.seq()),
        time: event.time(),
        data: event.data().clone(),
        ignorable: event.ignorable().then_some(true),
        r#type: event_type,
        surface_op: if surface || assistant || opaque {
            event.surface_op().cloned()
        } else {
            None
        },
        source_event_seqs: if surface || opaque {
            event.source_event_seqs().cloned()
        } else {
            None
        },
    }
}

fn json_bytes<T: Serialize>(value: &T) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |text| text.len())
}

struct BlockedWarning {
    header: Weak<Value>,
    blocked_warning: Option<SessionSeq>,
}

struct ExclusionEntry {
    id: u64,
    exclude: Arc<SessionLogUploadExclusion>,
}

struct Inner {
    ctx: Context,
    sessions: hoh_session::SessionStore,
    config: Cfg<Config>,
    max_bytes: usize,
    blocked_warnings: Mutex<Vec<BlockedWarning>>,
    exclusions: Mutex<Vec<ExclusionEntry>>,
    next_exclusion: AtomicU64,
}

/// Incremental Session-log contributor with caller-owned upload exclusions.
#[derive(Clone)]
pub struct SessionLogDeepSeekService(Arc<Inner>);
hoh_kernel::service!(pub SessionLogDeepSeek: SessionLogDeepSeekService = "sessionLogDeepSeek");

impl SessionLogDeepSeekService {
    fn fold(&self, session: &Session) -> Result<SessionSeqCursor, BoxError> {
        accepted_through(session)
    }

    fn warn_blocked(&self, session: &Session, first: SessionSeq, candidate_bytes: usize) {
        let header = session.header();
        let weak = Arc::downgrade(&header);
        let mut cache = lock(&self.0.blocked_warnings);
        cache.retain(|entry| entry.header.strong_count() != 0);
        let position = cache
            .iter()
            .position(|entry| Weak::ptr_eq(&entry.header, &weak));
        let entry = match position {
            Some(position) => &mut cache[position],
            None => {
                cache.push(BlockedWarning {
                    header: weak,
                    blocked_warning: None,
                });
                cache.last_mut().expect("just pushed a blocked warning")
            }
        };
        if entry.blocked_warning == Some(first) {
            return;
        }
        entry.blocked_warning = Some(first);
        if candidate_bytes == usize::MAX {
            self.0.ctx.logger().warn(format_args!(
                "session-log-deepseek: event {first} of session \"{}\" is too large to serialize into a dsh_session_log field; this session's upload stays at event {first}",
                session.id()
            ));
        } else {
            self.0.ctx.logger().warn(format_args!(
                "session-log-deepseek: event {first} of session \"{}\" needs a {candidate_bytes}-byte dsh_session_log field, above maxBytes {}; this session's upload stays at event {first} until maxBytes admits it",
                session.id(), self.0.max_bytes
            ));
        }
    }

    fn prepare(
        &self,
        request: &DeepSeekLlmApiExtensionRequest,
    ) -> Result<Option<PreparedDeepSeekLlmApiExtension<DeepSeekSessionLogExtension>>, BoxError>
    {
        if !*self.0.config.get().enabled {
            return Ok(None);
        }
        let Some(id) = request.session_id.as_deref() else {
            return Ok(None);
        };
        let Some(session) = self.0.sessions.get(&id.into()) else {
            return Ok(None);
        };
        let upload = SessionLogUploadRequest {
            session_id: session.id().to_string(),
            purpose: request.purpose.clone(),
        };
        let exclusions: Vec<_> = lock(&self.0.exclusions)
            .iter()
            .map(|entry| entry.exclude.clone())
            .collect();
        for exclude in exclusions {
            if exclude(&upload)? {
                return Ok(None);
            }
        }

        let after = self.fold(&session)?;
        let start = match after {
            SessionSeqCursor::BeforeFirst => 0,
            SessionSeqCursor::Seq(seq) => u64::from(seq) + 1,
        };
        let pending = session.snapshot_events(Some(SessionLogOffset::new(start)?), None);
        let header = wire_header(&session)?;
        let session_format_version = header.version;
        let base = DeepSeekSessionLogExtension {
            version: 1,
            session_format_version,
            session: header,
            after_seq: cursor_number(after),
            through_seq: 0,
            events: Vec::new(),
        };
        let mut bytes = json_bytes(&base).saturating_sub(1);
        let mut events = Vec::new();
        let mut candidate_bytes = 0;
        for event in pending.iter() {
            let wire = wire_event(event);
            let next = bytes
                .saturating_add(usize::from(!events.is_empty()))
                .saturating_add(json_bytes(&wire));
            candidate_bytes = next.saturating_add(event.seq().to_string().len());
            if candidate_bytes > self.0.max_bytes {
                break;
            }
            bytes = next;
            events.push(wire);
        }
        let Some(last) = events
            .len()
            .checked_sub(1)
            .and_then(|index| pending.get(index))
        else {
            if let Some(first) = pending.first() {
                self.warn_blocked(&session, first.seq(), candidate_bytes);
            }
            return Ok(None);
        };
        let through = last.seq();
        let value = DeepSeekSessionLogExtension {
            through_seq: u64::from(through),
            events,
            ..base
        };
        let accept: AcceptanceCallback = Arc::new(move || {
            let session = session.clone();
            async move {
                session.append(
                    DELIVERY_ACCEPTED,
                    json!({
                        "sessionId": session.id(),
                        "sessionFormatVersion": session_format_version,
                        "throughSeq": through,
                    }),
                    None,
                )?;
                Ok(())
            }
            .boxed()
        });
        Ok(Some(PreparedDeepSeekLlmApiExtension {
            value,
            accept: Some(accept),
        }))
    }

    /// Register a synchronous exclusion owned by `ctx`.
    pub fn register_exclusion<F>(&self, ctx: &Context, exclude: F) -> Result<Disposer, BoxError>
    where
        F: Fn(&SessionLogUploadRequest) -> Result<bool, BoxError> + Send + Sync + 'static,
    {
        let id = self.0.next_exclusion.fetch_add(1, Ordering::Relaxed);
        let inner = self.0.clone();
        let exclude: Arc<SessionLogUploadExclusion> = Arc::new(exclude);
        ctx.try_effect("sessionLogDeepSeek.registerExclusion()", move |undos| {
            lock(&inner.exclusions).push(ExclusionEntry { id, exclude });
            let cleanup = inner.clone();
            undos.push(Undo::sync(move || {
                lock(&cleanup.exclusions).retain(|entry| entry.id != id);
            }))?;
            Ok(())
        })
    }
}

#[derive(Inject)]
pub struct Deps {
    extensions: Service<DeepSeekLlmApiExtensions>,
    sessions: Service<Sessions>,
}

/// Plugin registering the `dsh_session_log` request field.
pub struct SessionLogDeepSeekPlugin;

impl Plugin for SessionLogDeepSeekPlugin {
    const NAME: &'static str = "SessionLogDeepSeekService";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let max_bytes = resolve_max_bytes(config.get().max_bytes)?;
        let service = SessionLogDeepSeekService(Arc::new(Inner {
            ctx: ctx.clone(),
            sessions: (*deps.sessions).clone(),
            config,
            max_bytes,
            blocked_warnings: Mutex::new(Vec::new()),
            exclusions: Mutex::new(Vec::new()),
            next_exclusion: AtomicU64::new(0),
        }));
        let provider = service.clone();
        deps.extensions.register::<DeepSeekSessionLogExtension, _>(
            &ctx,
            "dsh_session_log",
            move |request: Arc<DeepSeekLlmApiExtensionRequest>| {
                let result = provider.prepare(&request);
                async move { result }
            },
        )?;
        ctx.provide::<SessionLogDeepSeek>(Arc::new(service)).await?;
        Ok(())
    }
}
