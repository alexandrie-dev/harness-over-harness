//! Agent-owned model selection, its durable projection, and scoped switch notices.

use crate::{
    runtime_types::{AgentOptions, AgentPreStep, Model, PreStepDecision},
    types::ModelSelection,
};
use hoh_error::BoxError;
use hoh_kernel::{Context, Disposer, KernelError};
use hoh_llm::{
    ContentBlock, MessageSource, NewUserMessage, ReasoningEffortId, bound_context_summary,
    create_user_message,
};
use hoh_session::{Session, SessionEvent};
use hoh_session_projection::{ProjectionDefinition, ProjectionWire};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// Durable model-selection state for one Session. An effort the adapter defaulted on a recorded
/// request is omitted, so later requests resolve the default again instead of pinning it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelectionProjectionState {
    /// Selection used by the latest recorded model request.
    pub last_used: Option<ModelSelection>,
    /// Later recorded selection not yet used by a matching model request.
    pub pending: Option<ModelSelection>,
}

/// `selectionSchema` of the source: `null`, or an object with non-empty `provider` and `model`
/// and an optional (never `null`) non-empty `reasoningEffort`.
fn parse_selection(value: &Value, field: &str) -> Result<Option<ModelSelection>, BoxError> {
    if value.is_null() {
        return Ok(None);
    }
    let invalid = |what: &str| format!("invalid modelSelection projection: {field}.{what}");
    let object = value
        .as_object()
        .ok_or_else(|| invalid("expected an object or null"))?;
    let text = |key: &str| match object.get(key) {
        Some(Value::String(text)) if !text.is_empty() => Ok(text.clone()),
        _ => Err(invalid(&format!("{key} must be a non-empty string"))),
    };
    let reasoning_effort = match object.get("reasoningEffort") {
        None => None,
        Some(Value::String(text)) if !text.is_empty() => Some(ReasoningEffortId::from(&**text)),
        Some(_) => return Err(invalid("reasoningEffort must be a non-empty string").into()),
    };
    Ok(Some(ModelSelection {
        provider: text("provider")?,
        model: text("model")?,
        reasoning_effort,
    }))
}

/// Both keys are required: the schema of the source has no defaults for a missing key.
fn required<'a>(
    object: &'a serde_json::Map<String, Value>,
    key: &str,
) -> Result<&'a Value, BoxError> {
    object
        .get(key)
        .ok_or_else(|| format!("invalid modelSelection projection: {key} is required").into())
}

fn object_of(raw: Option<&Value>) -> Result<&serde_json::Map<String, Value>, BoxError> {
    raw.ok_or("persisted projection state is missing")?
        .as_object()
        .ok_or_else(|| "invalid modelSelection projection: expected an object".into())
}

fn to_selection(
    provider: &str,
    model: &str,
    reasoning_effort: Option<ReasoningEffortId>,
) -> ModelSelection {
    ModelSelection {
        provider: provider.to_owned(),
        model: model.to_owned(),
        reasoning_effort,
    }
}

fn same_route(left: &ModelSelection, right: &ModelSelection) -> bool {
    left.provider == right.provider && left.model == right.model
}

/// Compare complete selections, treating absent effort as its own value.
fn same_model_selection(left: Option<&ModelSelection>, right: Option<&ModelSelection>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            same_route(left, right) && left.reasoning_effort == right.reasoning_effort
        }
        _ => false,
    }
}

/// Selection a recorded request used, omitting an effort the adapter defaulted.
fn selection_of_header(header: &Value) -> Result<ModelSelection, BoxError> {
    let config = header
        .get("config")
        .ok_or("request header without config")?;
    let text = |key: &str| config.get(key).and_then(|v| v.as_str());
    let defaulted = header
        .get("adapterDefaults")
        .and_then(|d| d.get("reasoningEffort"))
        == Some(&Value::Bool(true));
    let effort = if defaulted {
        None
    } else {
        text("reasoningEffort").map(ReasoningEffortId::from)
    };
    Ok(to_selection(
        text("provider").ok_or("request header config without provider")?,
        text("model").ok_or("request header config without model")?,
        effort,
    ))
}

/// Session selection fold: recorded selections become the pending choice, and each request
/// header retires a matching one (`modelSelection`, state version 3).
pub fn model_selection_projection_definition() -> ProjectionDefinition<ModelSelectionProjectionState>
{
    let mut definition = crate::projection::fold_definition(
        "modelSelection",
        3,
        ModelSelectionProjectionState::default,
        |state: &ModelSelectionProjectionState, event: &SessionEvent| match event.event_type() {
            "model/selection" => {
                let data: ModelSelection = serde_json::from_value(event.data().clone())?;
                if same_model_selection(state.pending.as_ref(), Some(&data)) {
                    Ok(state.clone())
                } else {
                    Ok(ModelSelectionProjectionState {
                        pending: Some(data),
                        ..state.clone()
                    })
                }
            }
            "request/header" => {
                let header = event
                    .data()
                    .get("header")
                    .ok_or("request/header without header")?;
                let last_used = selection_of_header(header)?;
                let pending = if same_model_selection(state.pending.as_ref(), Some(&last_used)) {
                    None
                } else {
                    state.pending.clone()
                };
                if same_model_selection(state.last_used.as_ref(), Some(&last_used))
                    && pending == state.pending
                {
                    Ok(state.clone())
                } else {
                    Ok(ModelSelectionProjectionState {
                        last_used: Some(last_used),
                        pending,
                    })
                }
            }
            _ => Ok(state.clone()),
        },
    );
    definition.state_schema = Arc::new(|raw| {
        let object = object_of(raw)?;
        Ok(Arc::new(ModelSelectionProjectionState {
            last_used: parse_selection(required(object, "lastUsed")?, "lastUsed")?,
            pending: parse_selection(required(object, "pending")?, "pending")?,
        }))
    });
    definition.wire = Some(ProjectionWire::new(
        Arc::new(|raw| {
            let object = object_of(raw)?;
            parse_selection(required(object, "lastUsed")?, "lastUsed")?;
            parse_selection(required(object, "next")?, "next")?;
            Ok(Arc::new(Value::Object(object.clone())))
        }),
        Arc::new(|state| {
            Ok(Some(Arc::new(serde_json::to_value(model_selection_view(
                state,
            ))?)))
        }),
    ));
    definition
}

/// Wire view of the fold: `{ lastUsed, next }` (`next` falls back to `lastUsed`).
pub fn model_selection_view(
    state: &ModelSelectionProjectionState,
) -> crate::types::ModelSelectionProjection {
    crate::types::ModelSelectionProjection {
        last_used: state.last_used.clone(),
        next: state.pending.clone().or_else(|| state.last_used.clone()),
    }
}

type ReadState = Arc<dyn Fn() -> Arc<ModelSelectionProjectionState> + Send + Sync>;

/// Read and record one Agent's model choices without resolving adapter defaults.
pub struct AgentModel {
    read_state: ReadState,
    session: Session,
    options: AgentOptions,
}

impl AgentModel {
    /// * `read_state` — driver-owned reader for this Session's selection projection.
    /// * `session` — Session receiving durable model selections.
    /// * `options` — live route fallback until a selection or request is recorded.
    pub fn new(read_state: ReadState, session: Session, options: AgentOptions) -> Self {
        Self {
            read_state,
            session,
            options,
        }
    }

    fn recorded_selection(&self) -> Option<ModelSelection> {
        let state = (self.read_state)();
        state.pending.clone().or_else(|| state.last_used.clone())
    }
}

impl Model for AgentModel {
    fn current(&self) -> Option<ModelSelection> {
        if let Some(recorded) = self.recorded_selection() {
            return Some(recorded);
        }
        let (provider, model) = (self.options.provider(), self.options.model());
        match (provider, model) {
            (Some(provider), Some(model)) if !provider.is_empty() && !model.is_empty() => Some(
                to_selection(&provider, &model, self.options.reasoning_effort()),
            ),
            _ => None,
        }
    }

    fn select(&self, selection: &ModelSelection) -> Result<(), BoxError> {
        if same_model_selection(self.recorded_selection().as_ref(), Some(selection)) {
            return Ok(());
        }
        let data = serde_json::to_value(to_selection(
            &selection.provider,
            &selection.model,
            selection.reasoning_effort.clone(),
        ))?;
        self.session.append("model/selection", data, None)?;
        Ok(())
    }
}

fn route_label(route: &ModelSelection, other: &ModelSelection) -> String {
    if route.provider == other.provider {
        route.model.clone()
    } else {
        format!("{}/{}", route.provider, route.model)
    }
}

fn model_switch_notice(
    previous: &ModelSelection,
    selected: &ModelSelection,
) -> hoh_llm::UserMessage {
    let from = route_label(previous, selected);
    let to = route_label(selected, previous);
    let mut source = MessageSource::new("model-selection");
    source.data.insert("form".into(), "notice".into());
    source.data.insert(
        "summary".into(),
        bound_context_summary(&format!("{from} → {to}")).into(),
    );
    create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(format!(
            "[model changed: assistant turns above this point were generated by {from}; the session continues with {to}]"
        ))],
        source,
    })
}

/// Append a model-switch notice when the step's fixed route differs from the last request header.
/// Effort-only changes and empty no-request decisions add no notice; a failure before header
/// persistence repeats it on the next step. Install in the selected Agent's unpublished setup scope.
pub fn install_model_selection(agent_ctx: &Context) -> Result<Disposer, KernelError> {
    agent_ctx
        .listen()
        .prepend()
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(|event, next| async move {
            let decision = next.run().await?;
            let PreStepDecision::Enter {
                messages,
                starts_request_series,
            } = decision
            else {
                return Ok(decision);
            };
            if event.signal.aborted() {
                return Ok(PreStepDecision::Enter {
                    messages,
                    starts_request_series,
                });
            }
            let keep = PreStepDecision::Enter {
                messages: messages.clone(),
                starts_request_series,
            };
            // The loop skips an empty first step and an emptied offered continuation.
            if messages.is_empty() && (event.step == 1 || !event.messages.is_empty()) {
                return Ok(keep);
            }
            let Some(header) = event.agent.session().request_header() else {
                return Ok(keep);
            };
            let Some(previous) = header.get("config").and_then(|config| {
                serde_json::from_value::<ModelSelectionRoute>(config.clone()).ok()
            }) else {
                return Ok(keep);
            };
            if event.model_selection.provider == previous.provider
                && event.model_selection.model == previous.model
            {
                return Ok(keep);
            }
            let previous = to_selection(&previous.provider, &previous.model, None);
            let mut messages = messages;
            messages.push(model_switch_notice(&previous, &event.model_selection));
            Ok(PreStepDecision::Enter {
                messages,
                starts_request_series,
            })
        })
}

#[derive(serde::Deserialize)]
struct ModelSelectionRoute {
    provider: String,
    model: String,
}
