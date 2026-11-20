use hoh_agent::{Agent, AgentCreated, Agents};
use hoh_error::BoxError;
use hoh_kernel::{Context, Event, Serial};
use hoh_llm::{
    ContentBlock, MessageSource, NewUserMessage, TokenUsage, create_user_message,
    expand_assistant_stream_value,
};
use hoh_session::{SessionEvent, SessionEventEmitted, SessionId, Sessions};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex, MutexGuard};

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Observer called for canonical events in the fixture turn's owned interval.
pub type FixtureEventObserver = Arc<dyn Fn(&SessionId, &SessionEvent) + Send + Sync + 'static>;

/// Options for one fixture turn against exactly one configured root agent.
#[derive(Clone, Default)]
pub struct FixtureTurnOptions {
    pub task: String,
    pub on_event: Option<FixtureEventObserver>,
}

/// Result envelope consumed by snapshot and composition tests.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureTurnResult {
    #[serde(rename = "type")]
    pub r#type: String,
    pub session_id: SessionId,
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
}

fn is_root(agent: &Arc<dyn Agent>) -> bool {
    agent.session().header()["origin"].as_str() != Some("subagent")
}

async fn wait_for_serial_event<E, R>(
    ctx: &Context,
    should_wait: impl FnOnce() -> bool,
    registered: impl FnOnce(),
    observed: impl Fn() + Send + Sync + 'static,
) -> Result<(), BoxError>
where
    E: Event<Mode = Serial<R>>,
    R: Send + 'static,
{
    let (send, receive) = tokio::sync::oneshot::channel();
    let send = Arc::new(Mutex::new(Some(send)));
    let disposer = ctx.on_serial::<E, R, _>(move |_| {
        observed();
        let send = send.clone();
        async move {
            if let Some(send) = lock(&send).take() {
                let _ = send.send(());
            }
            Ok(None)
        }
    })?;
    registered();
    if should_wait() {
        let _ = receive.await;
    }
    disposer.dispose().await;
    Ok(())
}

async fn only_root_agent(ctx: &Context) -> Result<Arc<dyn Agent>, BoxError> {
    let registry = ctx.get::<Agents>().ok_or_else(|| -> BoxError {
        "fixture turn requires exactly one top-level agent, found 0".into()
    })?;
    if registry.list().into_iter().filter(is_root).count() == 0 {
        // JavaScript cannot publish between the empty-list check and listener registration;
        // native threads can, so close that window with a post-registration check.
        let recheck = registry.clone();
        wait_for_serial_event::<AgentCreated, ()>(
            ctx,
            move || recheck.list().into_iter().filter(is_root).count() == 0,
            || {},
            || {},
        )
        .await?;
    }
    let roots: Vec<_> = registry.list().into_iter().filter(is_root).collect();
    match roots.as_slice() {
        [agent] => Ok(agent.clone()),
        _ => Err(format!(
            "fixture turn requires exactly one top-level agent, found {}",
            roots.len()
        )
        .into()),
    }
}

fn add_usage(total: Option<TokenUsage>, step: TokenUsage) -> TokenUsage {
    let mut next = TokenUsage {
        input_tokens: total.as_ref().map_or(0, |usage| usage.input_tokens) + step.input_tokens,
        output_tokens: total.as_ref().map_or(0, |usage| usage.output_tokens) + step.output_tokens,
        ..Default::default()
    };
    macro_rules! optional_sum {
        ($field:ident) => {
            if total.as_ref().and_then(|usage| usage.$field).is_some() || step.$field.is_some() {
                next.$field = Some(
                    total.as_ref().and_then(|usage| usage.$field).unwrap_or(0)
                        + step.$field.unwrap_or(0),
                );
            }
        };
    }
    optional_sum!(cache_read_tokens);
    optional_sum!(cache_write_tokens);
    optional_sum!(reasoning_tokens);
    next
}

fn assistant_text(event: &SessionEvent) -> Option<String> {
    let blocks = event.data().get("message")?.get("content")?.as_array()?;
    let texts: Vec<_> = blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect();
    (!texts.is_empty()).then(|| texts.concat())
}

fn attempt_usage(event: &SessionEvent) -> Result<Option<TokenUsage>, BoxError> {
    let stream = event.data().get("stream").unwrap_or(&Value::Null);
    let expanded = expand_assistant_stream_value(stream)?;
    let Some(chunk) = expanded.iter().rev().find_map(|member| {
        member
            .get("chunk")
            .filter(|chunk| chunk.get("type").and_then(Value::as_str) == Some("usage"))
    }) else {
        return Ok(None);
    };
    Ok(Some(serde_json::from_value(
        chunk.get("usage").cloned().unwrap_or(Value::Null),
    )?))
}

#[derive(Default)]
struct TurnState {
    received: bool,
    output: String,
    usage_by_step: indexmap::IndexMap<String, TokenUsage>,
}

/// Drive one task from durable inbox receipt through whole-agent idle.
pub async fn run_fixture_turn(
    ctx: &Context,
    options: FixtureTurnOptions,
) -> Result<FixtureTurnResult, BoxError> {
    let agent = only_root_agent(ctx).await?;
    agent.when_idle().await;
    let message = create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(options.task)],
        source: MessageSource::new("user"),
    });
    let message_id = message.id.clone();
    let session = agent.session().clone();
    let session_id = session.id().clone();
    let state = Arc::new(Mutex::new(TurnState::default()));
    let listener_state = state.clone();
    let observer = options.on_event;
    let owned_session = session.clone();
    let listener = ctx.on_contained::<SessionEventEmitted>(move |emitted| {
        if emitted.session != owned_session {
            return Ok(());
        }
        let event = &emitted.event;
        let mut state = lock(&listener_state);
        if !state.received {
            if event.event_type() != "agent/inbox/spliced"
                || !event.data()["inserted"].as_array().is_some_and(|inserted| {
                    inserted.iter().any(|value| {
                        value.get("id") == Some(&Value::String(message_id.to_string()))
                    })
                })
            {
                return Ok(());
            }
            state.received = true;
        }
        if let Some(observer) = &observer {
            observer(&session_id, event);
        }
        let turn = event.data().get("turn").and_then(Value::as_u64);
        let step = event.data().get("step").and_then(Value::as_u64);
        match event.event_type() {
            "assistant/message" => {
                if let Some(output) = assistant_text(event) {
                    state.output = output;
                }
                if let (Some(turn), Some(step), Some(usage)) = (
                    turn,
                    step,
                    event
                        .data()
                        .get("usage")
                        .and_then(|value| serde_json::from_value(value.clone()).ok()),
                ) {
                    state.usage_by_step.insert(format!("{turn}/{step}"), usage);
                }
            }
            "assistant/attempt" => {
                if let (Some(turn), Some(step), Some(usage)) = (turn, step, attempt_usage(event)?) {
                    state.usage_by_step.insert(format!("{turn}/{step}"), usage);
                }
            }
            _ => {}
        }
        Ok(())
    })?;

    let turn = async {
        agent.followup(message)?;
        agent.when_idle().await;
        Ok::<(), BoxError>(())
    }
    .await;
    listener.dispose().await;
    turn?;
    ctx.get::<Sessions>()
        .ok_or_else(|| -> BoxError { "fixture turn requires the sessions service".into() })?
        .flush(&session)
        .await?;
    let mut state = lock(&state);
    let usage = std::mem::take(&mut state.usage_by_step)
        .into_values()
        .fold(None, |total, step| Some(add_usage(total, step)));
    Ok(FixtureTurnResult {
        r#type: "result".into(),
        session_id: session.id().clone(),
        output: std::mem::take(&mut state.output),
        usage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct WaitProbe;
    hoh_kernel::event!(WaitProbe = "loader-smoke/test-wait-probe", Serial<()>);

    #[tokio::test]
    async fn one_shot_serial_wait_disposes_its_listener() {
        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        let (registered, ready) = tokio::sync::oneshot::channel();
        let observed = Arc::new(AtomicUsize::new(0));
        let counted = observed.clone();
        let waiting_ctx = ctx.clone();
        let waiting = tokio::spawn(async move {
            wait_for_serial_event::<WaitProbe, ()>(
                &waiting_ctx,
                || true,
                move || {
                    let _ = registered.send(());
                },
                move || {
                    counted.fetch_add(1, Ordering::SeqCst);
                },
            )
            .await
        });

        ready.await.unwrap();
        ctx.serial::<WaitProbe, ()>(WaitProbe).await.unwrap();
        waiting.await.unwrap().unwrap();
        ctx.serial::<WaitProbe, ()>(WaitProbe).await.unwrap();
        assert_eq!(observed.load(Ordering::SeqCst), 1);
        kernel.shutdown().await;
    }

    #[test]
    fn malformed_last_usage_does_not_fall_back_to_an_earlier_usage_chunk() {
        let event = SessionEvent::from_value(json!({
            "type": "assistant/attempt",
            "seq": 0,
            "time": 0,
            "data": {
                "stream": [
                    { "type": "chunk", "time": 1, "chunk": {
                        "type": "usage", "usage": { "inputTokens": 1, "outputTokens": 2 }
                    }},
                    { "type": "chunk", "time": 2, "chunk": {
                        "type": "usage", "usage": { "inputTokens": "broken", "outputTokens": 3 }
                    }}
                ]
            }
        }));
        assert!(attempt_usage(&event).is_err());
    }

    #[test]
    fn validates_packed_non_usage_records_before_reading_usage() {
        let event = SessionEvent::from_value(json!({
            "type": "assistant/attempt",
            "seq": 0,
            "time": 0,
            "data": {
                "stream": [
                    { "type": "text-chunks", "time0": 0, "index": 0,
                      "dt": [], "texts": ["a", "b"] },
                    { "type": "chunk", "time": 1, "chunk": {
                        "type": "usage", "usage": { "inputTokens": 1, "outputTokens": 2 }
                    }}
                ]
            }
        }));
        assert_eq!(
            attempt_usage(&event).unwrap_err().to_string(),
            "text-chunks dt length must be one less than its members"
        );
    }
}
