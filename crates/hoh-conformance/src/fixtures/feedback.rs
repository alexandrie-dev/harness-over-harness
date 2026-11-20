//! `snapshots/sdk/text-turn/feedback-producer.mjs`: exercise the real feedback services before the SDK
//! closes the recorded root turn.
use super::{SNAPSHOTS, is_child};
use hoh_agent::AgentTurnStopping;
use hoh_command_feedback::SessionFeedback;
use hoh_commands::{CommandResult, Commands};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_loader::PluginRegistry;
use hoh_message_feedback::MessageFeedback;
use serde_json::{Value, json};

#[derive(Inject)]
pub struct FeedbackDeps {
    commands: Service<Commands>,
    message_feedback: Service<MessageFeedback>,
    session_feedback: Service<SessionFeedback>,
}

/// `{ok: true, value}` → value; `{ok: false, error: {code}}` → the TypeScript `throw new Error(code)`.
fn settled(result: &impl serde::Serialize) -> Result<Value, BoxError> {
    let result = serde_json::to_value(result)?;
    if result["ok"] == true {
        return Ok(result["value"].clone());
    }
    Err(result["error"]["code"]
        .as_str()
        .unwrap_or("feedback request failed")
        .to_owned()
        .into())
}

pub struct FeedbackProducer;
impl Plugin for FeedbackProducer {
    const NAME: &'static str = "snapshot-feedback-producer";
    type Config = ();
    type Deps = FeedbackDeps;
    async fn apply(ctx: Context, deps: FeedbackDeps, _: Cfg<()>) -> Result<(), BoxError> {
        ctx.listen()
            .on_serial::<AgentTurnStopping, (), _>(move |event| {
                let commands = deps.commands.clone();
                let messages = deps.message_feedback.clone();
                let sessions = deps.session_feedback.clone();
                async move {
                    let agent = &event.agent;
                    if is_child(&agent.session().header()) {
                        return Ok(None);
                    }
                    let before = agent.session().derive_messages()?;
                    let message = before
                        .iter()
                        .rev()
                        .find(|m| m["role"] == "assistant")
                        .ok_or("feedback snapshot requires an assistant message")?;
                    let command = commands
                        .execute(
                            agent.clone(),
                            "/feedback The session needs a clearer explanation.",
                            vec![],
                            event.signal.clone(),
                        )
                        .await?;
                    if !matches!(
                        command.map(|c| c.result),
                        Some(CommandResult::Success { .. })
                    ) {
                        return Err("feedback command did not succeed".into());
                    }
                    let session_id = agent.id().as_str();
                    settled(
                        &sessions
                            .record(serde_json::from_value(
                                json!({"sessionId": session_id, "category": "other"}),
                            )?)
                            .await?,
                    )?;
                    let target = json!({"sessionId": session_id, "messageId": message["id"]});
                    let put = |extra: Value| {
                        let mut request = target.clone();
                        request
                            .as_object_mut()
                            .expect("object")
                            .extend(extra.as_object().expect("object").clone());
                        messages.put(serde_json::from_value(request).expect("put request"))
                    };
                    let created = settled(
                        &put(json!({"rating": "negative", "note": "Explain the result.",
                            "category": "task-result", "ifVersion": null}))
                        .await?,
                    )?;
                    let edited = settled(
                        &put(
                            json!({"rating": "positive", "note": "The explanation is clear now.",
                            "ifVersion": created["version"]}),
                        )
                        .await?,
                    )?;
                    let mut delete = target.clone();
                    delete["ifVersion"] = edited["version"].clone();
                    settled(&messages.delete(serde_json::from_value(delete)?).await?)?;
                    if serde_json::to_string(&agent.session().derive_messages()?)?
                        != serde_json::to_string(&before)?
                    {
                        return Err("feedback changed model-visible messages".into());
                    }
                    Ok(None)
                }
            })?;
        Ok(())
    }
}

pub fn add(registry: &mut PluginRegistry) {
    registry.add::<FeedbackProducer>(&format!("{SNAPSHOTS}sdk/text-turn/feedback-producer.mjs"));
}
