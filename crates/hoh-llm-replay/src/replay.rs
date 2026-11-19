use crate::{
    ReplayConfig, ReplayEntry, Result, SessionScript,
    adapter::ReplayAdapter,
    error, load_session_scripts, lock,
    script::{materialize_session_tokens, resolve_scripted_entry},
};
use hoh_deepseek_llm_api_extensions::{DeepSeekLlmApiExtensionRequest, DeepSeekLlmApiExtensions};
use hoh_kernel::{Context, Disposer};
use hoh_llm::{GenerateOptions, LlmError, LlmFailure, LlmStream, LlmStreamEvent, StreamChunk};
use hoh_timeout::{AbortController, AbortSignal};
use indexmap::IndexMap;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
const ANON: &str = "\0anon\0";
struct Bound {
    script: usize,
    cursor: usize,
}
struct Bindings {
    scripts: Vec<SessionScript>,
    bound: IndexMap<String, Bound>,
    live_ids: Vec<Option<String>>,
    next: usize,
}
pub(crate) struct Replay {
    ctx: Context,
    bindings: Arc<Mutex<Bindings>>,
    pace_ms: f64,
}
enum Registration {
    Adapter(Box<hoh_llm::AdapterRegistrationHandle>),
    Listener(Disposer),
}
pub struct ReplayHandle {
    registration: Mutex<Option<Registration>>,
    bindings: Arc<Mutex<Bindings>>,
}
impl ReplayHandle {
    pub fn dispose(&self) {
        if let Some(registration) = lock(&self.registration).take() {
            match registration {
                Registration::Adapter(handle) => handle.dispose(),
                Registration::Listener(disposer) => drop(disposer.dispose()),
            }
        }
    }
    pub fn assert_consumed(&self) -> Result<()> {
        let state = lock(&self.bindings);
        let mut problems = vec![];
        if state.next < state.scripts.len() {
            problems.push(format!(
                "{} recorded script(s) never bound to a live session",
                state.scripts.len() - state.next
            ));
        }
        for (key, bound) in &state.bound {
            let length = state.scripts[bound.script].entries.len();
            if bound.cursor < length {
                let who = if key == ANON {
                    "the anonymous session".into()
                } else {
                    format!("session {key}")
                };
                problems.push(format!(
                    "{who} consumed {}/{length} recorded call(s)",
                    bound.cursor
                ));
            }
        }
        if !problems.is_empty() {
            return Err(error(format!(
                "llm-replay: fixture not fully consumed — {}; the scenario drove fewer model calls than recorded",
                problems.join("; ")
            )));
        }
        Ok(())
    }
}
pub fn install_llm_replay(ctx: &Context, config: &ReplayConfig) -> Result<ReplayHandle> {
    validate_pace(config.pace_ms)?;
    install_scripts(ctx, config, load_session_scripts(config)?)
}
pub(crate) fn validate_pace(value: Option<f64>) -> Result<()> {
    let pace = value.unwrap_or(0.0);
    if !pace.is_finite() || pace < 0.0 || pace.fract() != 0.0 {
        return Err(error(format!(
            "llm-replay: paceMs must be a non-negative integer, got {}",
            value
                .map(hoh_util_values::js_number_string)
                .unwrap_or_else(|| "undefined".into())
        )));
    }
    Ok(())
}
pub(crate) fn install_scripts(
    ctx: &Context,
    config: &ReplayConfig,
    scripts: Vec<SessionScript>,
) -> Result<ReplayHandle> {
    validate_pace(config.pace_ms)?;
    let bindings = Arc::new(Mutex::new(Bindings {
        live_ids: vec![None; scripts.len()],
        scripts,
        bound: IndexMap::new(),
        next: 0,
    }));
    let replay = Arc::new(Replay {
        ctx: ctx.clone(),
        bindings: bindings.clone(),
        pace_ms: config.pace_ms.unwrap_or(0.0),
    });
    let providers = config.providers.clone().unwrap_or_default();
    let registration = if !providers.is_empty() {
        let llm = ctx
            .get::<hoh_llm::Llm>()
            .ok_or_else(|| error("llm-replay: llm service is required"))?;
        Registration::Adapter(Box::new(llm.register_adapter(
            ctx,
            providers.iter().map(|p| p.id.clone()).collect(),
            Arc::new(ReplayAdapter { providers, replay }),
        )?))
    } else {
        Registration::Listener(ctx.on_waterfall::<LlmStreamEvent, LlmStream, _>(
            move |event, _next| {
                let stream = replay.stream(event.options());
                async move { Ok(stream) }
            },
        )?)
    };
    Ok(ReplayHandle {
        registration: Mutex::new(Some(registration)),
        bindings,
    })
}
impl Replay {
    pub(crate) fn stream(&self, options: GenerateOptions) -> LlmStream {
        let key = options
            .session_id
            .as_ref()
            .map(|s| s.as_str())
            .unwrap_or(ANON)
            .to_owned();
        let (entry, problem) = {
            let mut state = lock(&self.bindings);
            if !state.bound.contains_key(&key) {
                if state.next == state.scripts.len() {
                    let problem = error(format!(
                        "llm-replay: a model call arrived from an unrecorded session (#{}); the scenario recorded only {} session(s) — re-record it",
                        state.next + 1,
                        state.scripts.len()
                    ));
                    return make_stream(self, options, None, Some(problem));
                }
                let index = state.next;
                state.next += 1;
                if key != ANON {
                    state.live_ids[index] = Some(key.clone());
                }
                state.bound.insert(
                    key.clone(),
                    Bound {
                        script: index,
                        cursor: 0,
                    },
                );
            }
            // The new-session branch inserts before cursor access.
            let bound = state.bound.get_mut(&key).expect("session was bound");
            let index = bound.cursor;
            bound.cursor += 1;
            let script = bound.script;
            let entries = &state.scripts[script].entries;
            let entry = entries.get(index).cloned();
            let problem=entry.is_none().then(||error(format!("llm-replay: script exhausted — session requested model call #{} but its script has only {}; re-record the scenario",index+1,entries.len())));
            (entry, problem)
        };
        make_stream(self, options, entry, problem)
    }
}
struct Playing {
    ctx: Context,
    bindings: Arc<Mutex<Bindings>>,
    options: GenerateOptions,
    entry: Option<ReplayEntry>,
    problem: Option<hoh_error::BoxError>,
    prepared: bool,
    index: usize,
    pace_ms: f64,
}
fn make_stream(
    replay: &Replay,
    options: GenerateOptions,
    entry: Option<ReplayEntry>,
    problem: Option<hoh_error::BoxError>,
) -> LlmStream {
    let state = Playing {
        ctx: replay.ctx.clone(),
        bindings: replay.bindings.clone(),
        options,
        entry,
        problem,
        prepared: false,
        index: 0,
        pace_ms: replay.pace_ms,
    };
    LlmStream::new(futures_util::stream::try_unfold(
        state,
        |mut state| async move {
            if let Some(problem) = state.problem.take() {
                return Err(problem);
            }
            if !state.prepared {
                let ids = lock(&state.bindings).live_ids.clone();
                let entry = state
                    .entry
                    .take()
                    .ok_or_else(|| error("llm-replay: missing selected entry"))?;
                let materialized = materialize_session_tokens(&entry, &ids)?;
                let entry =
                    resolve_scripted_entry(&materialized, &state.options.messages)?.into_owned();
                let accepted = match &entry {
                    ReplayEntry::Chunks { .. } | ReplayEntry::Hang { .. } => true,
                    ReplayEntry::Throw {
                        accepted, chunks, ..
                    } => accepted.unwrap_or(!chunks.is_empty()),
                    ReplayEntry::Unvalidated(value) => match value["kind"].as_str() {
                        Some("chunks") => true,
                        Some("throw") => value["accepted"].as_bool().unwrap_or_else(|| {
                            value["chunks"].as_array().is_some_and(|c| !c.is_empty())
                        }),
                        _ => return Err(error("llm-replay: invalid unvalidated replay entry")),
                    },
                };
                if state.options.provider == "deepseek-official"
                    && accepted
                    && let Some(extensions) = state.ctx.get::<DeepSeekLlmApiExtensions>()
                {
                    let signal = state
                        .options
                        .signal
                        .clone()
                        .unwrap_or_else(|| AbortController::new().signal());
                    let prepared = extensions
                        .prepare(DeepSeekLlmApiExtensionRequest {
                            body: serde_json::Map::from_iter([(
                                "messages".into(),
                                serde_json::json!([]),
                            )]),
                            session_id: state.options.session_id.as_ref().map(ToString::to_string),
                            purpose: state.options.purpose.clone(),
                            signal,
                        })
                        .await?;
                    prepared.accept().await?;
                }
                state.entry = Some(entry);
                state.prepared = true;
            }
            // Preparation stores the selected entry before playback.
            let entry = state.entry.as_ref().expect("entry prepared");
            match entry {
                ReplayEntry::Chunks { .. }
                | ReplayEntry::Throw { .. }
                | ReplayEntry::Unvalidated(_) => {
                    let chunk = match entry {
                        ReplayEntry::Chunks { chunks } | ReplayEntry::Throw { chunks, .. } => {
                            chunks.get(state.index).cloned()
                        }
                        ReplayEntry::Unvalidated(value) => value["chunks"]
                            .get(state.index)
                            .map(|chunk| serde_json::from_value(chunk.clone()))
                            .transpose()?,
                        // The enclosing match excludes hang entries.
                        ReplayEntry::Hang { .. } => unreachable!("hang has its own playback"),
                    };
                    if let Some(chunk) = chunk {
                        if state
                            .options
                            .signal
                            .as_ref()
                            .is_some_and(AbortSignal::aborted)
                        {
                            return Err(error("aborted"));
                        }
                        if state.pace_ms > 0.0 {
                            let delay = if state.pace_ms > 2_147_483_647.0 {
                                1.0
                            } else {
                                state.pace_ms
                            };
                            let wait = state.ctx.sleep(Duration::from_secs_f64(delay / 1000.0));
                            if let Some(signal) = &state.options.signal {
                                tokio::select! {_ = signal.cancelled()=>return Err(error("aborted")),result=wait=>result?,}
                            } else {
                                wait.await?;
                            }
                        }
                        state.index += 1;
                        Ok(Some((chunk, state)))
                    } else if let ReplayEntry::Throw { message, code, .. } = entry {
                        Err(LlmError::from_failure(&LlmFailure::new(message, code), None)?.into())
                    } else if let ReplayEntry::Unvalidated(value) = entry
                        && value["kind"] == "throw"
                    {
                        let message = value["message"]
                            .as_str()
                            .ok_or_else(|| error("llm-replay: unvalidated throw lacks message"))?;
                        let code = value["code"]
                            .as_str()
                            .ok_or_else(|| error("llm-replay: unvalidated throw lacks code"))?;
                        Err(LlmError::from_failure(&LlmFailure::new(message, code), None)?.into())
                    } else {
                        Ok(None)
                    }
                }
                ReplayEntry::Hang { ready_file } => {
                    let chunk = match state.index {
                        0 => StreamChunk::BlockStart {
                            index: 0,
                            block_type: "text".into(),
                            wire: Default::default(),
                        },
                        1 => StreamChunk::TextDelta {
                            index: 0,
                            text: "partial".into(),
                            wire: Default::default(),
                        },
                        _ => {
                            if let Some(file) = ready_file {
                                tokio::fs::write(file, "")
                                    .await
                                    .map_err(|e| crate::fs_error(e, file))?;
                            }
                            let cancel = state.ctx.token();
                            if let Some(signal) = &state.options.signal {
                                tokio::select! {_=signal.cancelled()=>{},_=cancel.cancelled()=>{},}
                            } else {
                                cancel.cancelled().await;
                            }
                            return Err(error("aborted"));
                        }
                    };
                    state.index += 1;
                    Ok(Some((chunk, state)))
                }
            }
        },
    ))
}
