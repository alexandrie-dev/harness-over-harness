//! Opt-in durable request context describing this process's tmux location.

use futures_util::FutureExt;
use hoh_agent::{AgentPreStep, Agents, PreStepDecision};
use hoh_error::{BoxError, SharedError};
use hoh_kernel::{Cfg, Context, Inject, Logger, Plugin, Service};
use hoh_llm::{
    ContentBlock, ContextFormed, ContextSnapshotSection, MessageSource, NewUserMessage,
    create_user_message,
};
use hoh_session::{MAX_SAFE_INTEGER, SessionEvent};
use hoh_session_projection::{ProjectionDefinition, ProjectionHandle, SessionProjections};
use hoh_shell::{Shell, ShellExecRequest, ShellExecutor, ShellRunResult};
use hoh_util_values::{js_json_stringify, js_number_string};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    future::Future,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;

/// Cordis/Loader plugin name.
pub const NAME: &str = "tmux-context";

/// Service names declared by the source plugin.
pub const INJECT: [&str; 2] = ["agents", "sessionProjections"];

const FIELD_SEP: &str = "\\t";
const READING_PREFIX: &str = "tmux location (turn ";
const TMUX_FIELDS: [&str; 8] = [
    "#{session_name}",
    "#{window_index}",
    "#{window_name}",
    "#{pane_index}",
    "#{pane_id}",
    "#{window_active}",
    "#{pane_active}",
    "#{window_layout}",
];

/// Per-turn tmux-location scheduling.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Minimum milliseconds between durable injections in one Session.
    #[serde(
        default,
        deserialize_with = "deserialize_refresh_interval",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "f64")]
    pub refresh_interval_ms: Option<f64>,
}

fn deserialize_refresh_interval<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    f64::deserialize(deserializer).map(Some)
}

/// JavaScript-compatible configuration failure.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct TmuxContextError {
    message: String,
}

impl TmuxContextError {
    pub fn name(&self) -> &'static str {
        "TypeError"
    }
}

#[derive(Inject)]
pub struct Deps {
    agents: Service<Agents>,
    session_projections: Service<SessionProjections>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct TmuxContextState {
    state: String,
    time: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TmuxLocation {
    session_name: String,
    window_index: String,
    window_name: String,
    pane_index: String,
    pane_id: String,
    window_active: String,
    pane_active: String,
    window_layout: String,
}

type ProjectionOutcome = Result<ProjectionHandle<Option<TmuxContextState>>, SharedError>;

async fn await_projection(
    mut ready: watch::Receiver<Option<ProjectionOutcome>>,
) -> ProjectionOutcome {
    loop {
        if let Some(outcome) = ready.borrow().clone() {
            return outcome;
        }
        if ready.changed().await.is_err() {
            return Err(SharedError::new(BoxError::from(
                "tmux-context projection registration ended before settling",
            )));
        }
    }
}

fn validate_refresh_interval(value: Option<f64>) -> Result<Option<u64>, BoxError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if !value.is_finite()
        || value.fract() != 0.0
        || !(0.0..=MAX_SAFE_INTEGER as f64).contains(&value)
    {
        return Err(Box::new(TmuxContextError {
            message: format!(
                "tmux-context: refreshIntervalMs must be a non-negative safe integer, got {}",
                js_number_string(value)
            ),
        }));
    }
    Ok(Some(value as u64))
}

fn state_schema(value: Option<&Value>) -> Result<Arc<Option<TmuxContextState>>, BoxError> {
    let value = value
        .cloned()
        .ok_or_else(|| BoxError::from("tmux-context projection state is required"))?;
    if value.is_null() {
        return Ok(Arc::new(None));
    }
    Ok(Arc::new(Some(serde_json::from_value(value)?)))
}

fn fold_state(
    state: &Arc<Option<TmuxContextState>>,
    event: &SessionEvent,
) -> Result<Arc<Option<TmuxContextState>>, BoxError> {
    if event.event_type() != "user/message" || event.data()["source"]["kind"] != NAME {
        return Ok(state.clone());
    }
    let Some(block) = event.data()["content"]
        .as_array()
        .and_then(|blocks| blocks.first())
    else {
        return Ok(state.clone());
    };
    if block["type"] != "text" {
        return Ok(state.clone());
    }
    let Some(text) = block["text"].as_str() else {
        return Ok(state.clone());
    };
    let stable_state = text
        .find('\n')
        .map_or_else(String::new, |newline| text[newline + 1..].to_owned());
    Ok(Arc::new(Some(TmuxContextState {
        state: stable_state,
        time: event.time() as f64,
    })))
}

fn projection_definition() -> ProjectionDefinition<Option<TmuxContextState>> {
    ProjectionDefinition {
        key: "tmuxContext".into(),
        state_version: 1,
        state_schema: Arc::new(state_schema),
        init: Arc::new(|_, _| Ok(Arc::new(None))),
        apply: Arc::new(fold_state),
        wire: None,
    }
}

fn query_command(process_id: u32) -> String {
    let format = TMUX_FIELDS.join(FIELD_SEP);
    [
        "[ -n \"$TMUX_PANE\" ] || exit 1".to_owned(),
        format!("self_tty=$(ps -o tty= -p {process_id} | tr -d ' ')"),
        "[ -n \"$self_tty\" ] || exit 1".to_owned(),
        "pane_tty=$(tmux display-message -t \"$TMUX_PANE\" -p '#{pane_tty}') || exit 1".to_owned(),
        "[ \"$pane_tty\" = \"/dev/$self_tty\" ] || exit 1".to_owned(),
        format!("exec tmux display-message -t \"$TMUX_PANE\" -p '{format}'"),
    ]
    .join("\n")
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|value| (*value).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "shell executor panicked".to_owned())
}

async fn execute_query(
    shell: &dyn ShellExecutor,
    signal: hoh_timeout::AbortSignal,
) -> Result<ShellRunResult, String> {
    let request = ShellExecRequest {
        command: query_command(std::process::id()),
        workdir: None,
        timeout_ms: None,
        on_expiry: None,
        stdout_max_bytes: None,
        signal: Some(signal),
        stdin: None,
        env: None,
        dsh_env: None,
        sandbox_policy: None,
    };
    let spec = catch_unwind(AssertUnwindSafe(|| shell.resolve(&request))).map_err(panic_message)?;
    let execution_future =
        catch_unwind(AssertUnwindSafe(|| shell.execute(spec))).map_err(panic_message)?;
    let execution = AssertUnwindSafe(execution_future)
        .catch_unwind()
        .await
        .map_err(panic_message)?
        .map_err(|error| error.to_string())?;
    let result_future =
        catch_unwind(AssertUnwindSafe(|| execution.result())).map_err(panic_message)?;
    AssertUnwindSafe(result_future)
        .catch_unwind()
        .await
        .map_err(panic_message)?
        .map_err(|error| error.to_string())
}

async fn query_tmux_location(
    shell: &dyn ShellExecutor,
    logger: &Logger,
    signal: hoh_timeout::AbortSignal,
) -> Option<TmuxLocation> {
    let result = match execute_query(shell, signal).await {
        Ok(result) => result,
        Err(message) => {
            logger.warn(format_args!(
                "tmux location query failed: {message}; injecting no location this turn"
            ));
            return None;
        }
    };
    if result.exit_code != Some(0) {
        return None;
    }
    let line = result.stdout.text.split('\n').next().unwrap_or_default();
    let parts = line.split(FIELD_SEP).collect::<Vec<_>>();
    let [
        session_name,
        window_index,
        window_name,
        pane_index,
        pane_id,
        window_active,
        pane_active,
        window_layout,
    ] = parts.as_slice()
    else {
        return None;
    };
    if pane_id.is_empty() {
        return None;
    }
    Some(TmuxLocation {
        session_name: (*session_name).to_owned(),
        window_index: (*window_index).to_owned(),
        window_name: (*window_name).to_owned(),
        pane_index: (*pane_index).to_owned(),
        pane_id: (*pane_id).to_owned(),
        window_active: (*window_active).to_owned(),
        pane_active: (*pane_active).to_owned(),
        window_layout: (*window_layout).to_owned(),
    })
}

fn render_state(location: &TmuxLocation) -> String {
    let window_name = js_json_stringify(&Value::String(location.window_name.clone()));
    format!(
        "session {}, window {} {}, pane {} {}\nwindow active={}, pane active={}, layout {}",
        location.session_name,
        location.window_index,
        window_name,
        location.pane_index,
        location.pane_id,
        location.window_active,
        location.pane_active,
        location.window_layout
    )
}

fn render_reading(location: &TmuxLocation, turn: u64) -> String {
    format!("{READING_PREFIX}{turn}):\n{}", render_state(location))
}

fn tmux_context_message(text: String) -> hoh_llm::UserMessage {
    let mut source = MessageSource::new(NAME);
    let formed = ContextFormed::Snapshot {
        sections: vec![ContextSnapshotSection {
            name: NAME.into(),
            text: text.clone(),
        }],
    };
    if let Value::Object(data) = serde_json::to_value(formed).expect("context form is JSON") {
        source.data.extend(data);
    }
    create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source,
    })
}

fn now_ms() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis().min(i64::MAX as u128) as i64,
        Err(error) => -(error.duration().as_millis().min(i64::MAX as u128) as i64),
    }
}

/// Tmux location context plugin.
pub struct TmuxContext;

async fn apply_with_registration<F>(
    ctx: Context,
    refresh_interval_ms: Option<u64>,
    registration: F,
) -> Result<(), BoxError>
where
    F: Future<Output = Result<ProjectionHandle<Option<TmuxContextState>>, BoxError>> + Send,
{
    let (ready_tx, ready_rx) = watch::channel::<Option<ProjectionOutcome>>(None);
    let owner = ctx.clone();
    ctx.listen()
        .prepend()
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, next| {
            let owner = owner.clone();
            let ready = ready_rx.clone();
            async move {
                let projection = await_projection(ready)
                    .await
                    .map_err(|error| Box::new(error) as BoxError)?;
                owner.ensure_active()?;
                let decision = next.run().await?;
                let PreStepDecision::Enter {
                    mut messages,
                    starts_request_series,
                } = decision
                else {
                    return Ok(decision);
                };
                if event.signal.aborted() || event.step != 1 {
                    return Ok(PreStepDecision::Enter {
                        messages,
                        starts_request_series,
                    });
                }
                let Some(shell) = owner.get::<Shell>() else {
                    return Ok(PreStepDecision::Enter {
                        messages,
                        starts_request_series,
                    });
                };
                let previous = projection.state_of(event.agent.session())?;
                if let (Some(interval), Some(previous)) = (refresh_interval_ms, previous.as_ref()) {
                    let now = now_ms();
                    if interval > 0
                        && now as f64 >= previous.time
                        && now as f64 - previous.time < interval as f64
                    {
                        return Ok(PreStepDecision::Enter {
                            messages,
                            starts_request_series,
                        });
                    }
                }
                let Some(location) =
                    query_tmux_location(&*shell, &owner.logger(), event.signal.clone()).await
                else {
                    return Ok(PreStepDecision::Enter {
                        messages,
                        starts_request_series,
                    });
                };
                let state = render_state(&location);
                if previous
                    .as_ref()
                    .as_ref()
                    .is_some_and(|previous| previous.state == state)
                {
                    return Ok(PreStepDecision::Enter {
                        messages,
                        starts_request_series,
                    });
                }
                messages.insert(
                    0,
                    tmux_context_message(render_reading(&location, event.turn)),
                );
                Ok(PreStepDecision::Enter {
                    messages,
                    starts_request_series,
                })
            }
        })?;
    let outcome = registration.await.map_err(SharedError::new);
    let _ = ready_tx.send(Some(outcome.clone()));
    outcome
        .map(|_| ())
        .map_err(|error| Box::new(error) as BoxError)
}

impl TmuxContext {
    /// Installs the production listener behind a caller-controlled registration
    /// gate. This is a deterministic conformance seam for registration races.
    #[doc(hidden)]
    pub async fn apply_with_registration_gate<F>(
        ctx: Context,
        refresh_interval_ms: Option<f64>,
        gate: F,
    ) -> Result<(), BoxError>
    where
        F: Future<Output = Result<(), BoxError>> + Send,
    {
        let refresh_interval_ms = validate_refresh_interval(refresh_interval_ms)?;
        let projections = ctx
            .get::<SessionProjections>()
            .ok_or_else(|| BoxError::from("sessionProjections service is unavailable"))?;
        let registration_ctx = ctx.clone();
        let registration = async move {
            gate.await?;
            projections
                .register(&registration_ctx, projection_definition())
                .await
        };
        apply_with_registration(ctx, refresh_interval_ms, registration).await
    }
}

impl Plugin for TmuxContext {
    const NAME: &'static str = NAME;
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let refresh_interval_ms = validate_refresh_interval(config.get().refresh_interval_ms)?;
        let registration = deps
            .session_projections
            .register(&ctx, projection_definition());
        let _ = &deps.agents;
        apply_with_registration(ctx, refresh_interval_ms, registration).await
    }
}
